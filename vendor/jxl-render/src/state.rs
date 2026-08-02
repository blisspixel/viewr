use std::{
    collections::HashMap,
    sync::{Arc, Condvar, Mutex, MutexGuard},
};

use jxl_frame::data::{HfGlobal, LfGlobal, LfGroup};
use jxl_modular::{ChannelShift, Sample};

use crate::{
    Error, ImageWithRegion, IndexedFrame, Reference, Region, Result, image::RenderedImage,
};

pub type RenderOp<S> =
    Arc<dyn Fn(FrameRender<S>, Region) -> FrameRender<S> + Send + Sync + 'static>;

#[derive(Debug)]
pub struct RenderCache<S: Sample> {
    pub(crate) lf_global: Option<LfGlobal<S>>,
    pub(crate) hf_global: Option<HfGlobal>,
    pub(crate) lf_groups: HashMap<u32, LfGroup<S>>,
}

impl<S: Sample> RenderCache<S> {
    pub fn new(frame: &crate::IndexedFrame) -> Self {
        let frame_header = frame.header();
        let jpeg_upsampling = frame_header.jpeg_upsampling;
        let shifts_cbycr: [_; 3] =
            std::array::from_fn(|idx| ChannelShift::from_jpeg_upsampling(jpeg_upsampling, idx));

        let lf_width = frame_header.color_sample_width().div_ceil(8);
        let lf_height = frame_header.color_sample_height().div_ceil(8);
        let mut whd = [(lf_width, lf_height); 3];
        for ((w, h), shift) in whd.iter_mut().zip(shifts_cbycr) {
            let (shift_w, shift_h) = shift.shift_size((lf_width, lf_height));
            *w = shift_w;
            *h = shift_h;
        }
        Self {
            lf_global: None,
            hf_global: None,
            lf_groups: HashMap::new(),
        }
    }
}

#[derive(Default)]
pub enum FrameRender<S: Sample> {
    #[default]
    None,
    Rendering,
    InProgress(Box<RenderCache<S>>),
    Done(ImageWithRegion),
    Blended(Arc<ImageWithRegion>),
    Err(crate::Error),
    ErrTaken,
}

impl<S: Sample> std::fmt::Debug for FrameRender<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::Rendering => write!(f, "Rendering"),
            Self::InProgress(_) => write!(f, "InProgress(_)"),
            Self::Done(_) => write!(f, "Done(_)"),
            Self::Blended(_) => write!(f, "Blended(_)"),
            Self::Err(e) => f.debug_tuple("Err").field(e).finish(),
            Self::ErrTaken => write!(f, "ErrTaken"),
        }
    }
}

pub struct FrameRenderHandle<S: Sample> {
    pub(crate) frame: Arc<IndexedFrame>,
    pub(crate) image_region: Region,
    pub(crate) state: RenderState<S>,
    pub(crate) render_op: RenderOp<S>,
    pub(crate) refs: [Option<Reference<S>>; 4],
}

pub(crate) struct RenderState<S: Sample> {
    pub(crate) render: Mutex<FrameRender<S>>,
    condvar: Condvar,
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicBool,
}

impl<S: Sample> RenderState<S> {
    fn new(render: FrameRender<S>) -> Self {
        Self {
            render: Mutex::new(render),
            condvar: Condvar::new(),
            #[cfg(test)]
            waiting: std::sync::atomic::AtomicBool::new(false),
        }
    }

    fn wait_until_terminal(&self, frame_index: usize) -> Result<MutexGuard<'_, FrameRender<S>>> {
        let mut render_ref = self.render.lock().unwrap();
        loop {
            let render = std::mem::replace(&mut *render_ref, FrameRender::None);
            match render {
                FrameRender::Rendering => {
                    tracing::trace!(index = frame_index, "Waiting...");
                    *render_ref = render;
                    #[cfg(test)]
                    self.waiting
                        .store(true, std::sync::atomic::Ordering::Release);
                    render_ref = self.condvar.wait(render_ref).unwrap();
                    #[cfg(test)]
                    self.waiting
                        .store(false, std::sync::atomic::Ordering::Release);
                }
                FrameRender::Done(_) | FrameRender::Blended(_) => {
                    *render_ref = render;
                    return Ok(render_ref);
                }
                FrameRender::None | FrameRender::InProgress(_) => {
                    return Err(Error::IncompleteFrame);
                }
                FrameRender::Err(e) => return Err(e),
                FrameRender::ErrTaken => return Err(Error::FailedReference),
            }
        }
    }

    fn publish(&self, render: FrameRender<S>) -> MutexGuard<'_, FrameRender<S>> {
        assert!(!matches!(render, FrameRender::Rendering));
        let mut guard = self.render.lock().unwrap();
        *guard = render;
        self.condvar.notify_all();
        guard
    }

    pub(crate) fn publish_composition_error(&self) -> MutexGuard<'_, FrameRender<S>> {
        self.publish(FrameRender::ErrTaken)
    }
}

impl<S: Sample> std::fmt::Debug for FrameRenderHandle<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FrameRenderHandle")
            .field("render", &self.state.render)
            .finish_non_exhaustive()
    }
}

impl<S: Sample> FrameRenderHandle<S> {
    #[inline]
    pub fn new(
        frame: Arc<IndexedFrame>,
        image_region: Region,
        render_op: RenderOp<S>,
        refs: [Option<Reference<S>>; 4],
    ) -> Self {
        Self {
            frame,
            image_region,
            state: RenderState::new(FrameRender::None),
            render_op,
            refs,
        }
    }

    #[inline]
    pub fn from_cache(
        frame: Arc<IndexedFrame>,
        image_region: Region,
        cache: RenderCache<S>,
        render_op: RenderOp<S>,
        refs: [Option<Reference<S>>; 4],
    ) -> Self {
        let render = FrameRender::InProgress(Box::new(cache));
        Self {
            frame,
            image_region,
            state: RenderState::new(render),
            render_op,
            refs,
        }
    }

    pub fn run_with_image(self: Arc<Self>) -> Result<RenderedImage<S>> {
        let render = if let Some(state) = self.start_render()? {
            let _guard = tracing::trace_span!("Run with image", index = self.frame.idx).entered();

            let render_result = (self.render_op)(state, self.image_region);
            match render_result {
                FrameRender::InProgress(_) => {
                    drop(self.done_render(render_result));
                    return Err(Error::IncompleteFrame);
                }
                FrameRender::Err(e) => {
                    drop(self.done_render(FrameRender::ErrTaken));
                    return Err(e);
                }
                _ => {}
            }
            self.done_render(render_result)
        } else {
            self.wait_until_render()?
        };
        drop(render);

        Ok(RenderedImage::new(self))
    }

    pub fn run(&self, image_region: Region) {
        if let Some(state) = self.start_render_silent() {
            let _guard = tracing::trace_span!("Run", index = self.frame.idx).entered();

            let render_result = (self.render_op)(state, image_region);
            drop(self.done_render(render_result));
        }
    }

    pub fn reset(&self) -> FrameRender<S> {
        let mut render_ref = self.state.render.lock().unwrap();
        std::mem::replace(&mut *render_ref, FrameRender::None)
    }

    fn start_render(&self) -> Result<Option<FrameRender<S>>> {
        let mut render_ref = self.state.render.lock().unwrap();
        let render = std::mem::replace(&mut *render_ref, FrameRender::Rendering);
        match render {
            FrameRender::None | FrameRender::InProgress(_) => Ok(Some(render)),
            FrameRender::Err(e) => {
                *render_ref = FrameRender::ErrTaken;
                Err(e)
            }
            FrameRender::ErrTaken => {
                *render_ref = FrameRender::ErrTaken;
                Err(Error::FailedReference)
            }
            render => {
                *render_ref = render;
                Ok(None)
            }
        }
    }

    fn start_render_silent(&self) -> Option<FrameRender<S>> {
        let mut render_ref = self.state.render.lock().unwrap();
        let render = std::mem::replace(&mut *render_ref, FrameRender::Rendering);
        match render {
            FrameRender::None | FrameRender::InProgress(_) => Some(render),
            render => {
                *render_ref = render;
                None
            }
        }
    }

    pub(crate) fn wait_until_render(&self) -> Result<MutexGuard<'_, FrameRender<S>>> {
        self.state.wait_until_terminal(self.frame.idx)
    }

    pub(crate) fn done_render(&self, render: FrameRender<S>) -> MutexGuard<'_, FrameRender<S>> {
        self.state.publish(render)
    }
}

#[cfg(test)]
mod tests {
    use super::{FrameRender, RenderState};
    use crate::Error;
    use std::sync::Arc;
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    #[test]
    fn composition_error_transition_releases_a_render_waiter() {
        let state = Arc::new(RenderState::<i16>::new(FrameRender::Rendering));
        let waiter_state = Arc::clone(&state);
        let waiter = std::thread::spawn(move || {
            matches!(
                waiter_state.wait_until_terminal(0),
                Err(Error::FailedReference)
            )
        });
        let deadline = Instant::now() + Duration::from_secs(1);
        while !state.waiting.load(Ordering::Acquire) {
            assert!(
                Instant::now() < deadline,
                "render waiter did not reach the condition variable"
            );
            std::thread::yield_now();
        }

        drop(state.publish_composition_error());

        assert!(waiter.join().unwrap());
    }
}
