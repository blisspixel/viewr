#![allow(unsafe_code)]
use shared_memory::ShmemConf;
use std::io::{BufRead, Write};
use std::path::PathBuf;

fn main() {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout().lock();
    let mut lines = stdin.lock().lines();

    while let Some(Ok(path_str)) = lines.next() {
        let path_str = path_str.trim();
        if path_str.is_empty() {
            continue;
        }

        let path = PathBuf::from(path_str);
        
        match decode_file(&path) {
            Ok((width, height, rgba)) => {
                let size = rgba.len();
                let shmem_res = ShmemConf::new().size(size).create();
                
                match shmem_res {
                    Ok(shmem) => {
                        let slice = unsafe { std::slice::from_raw_parts_mut(shmem.as_ptr(), size) };
                        slice.copy_from_slice(&rgba);
                        
                        let name = shmem.get_os_id();
                        let _ = writeln!(stdout, "SHM {} {} {}", name, width, height);
                        let _ = stdout.flush();
                        
                        // Wait for ACK before dropping shmem
                        if let Some(Ok(ack)) = lines.next() {
                            if ack.trim() == "ACK" {
                                // main process consumed the memory, safe to drop
                            }
                        }
                    }
                    Err(e) => {
                        let _ = writeln!(stdout, "ERR failed to create shmem: {}", e);
                        let _ = stdout.flush();
                    }
                }
            }
            Err(e) => {
                let _ = writeln!(stdout, "ERR {}", e);
                let _ = stdout.flush();
            }
        }
    }
}

fn decode_file(path: &PathBuf) -> Result<(u32, u32, Vec<u8>), String> {
    let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    match ext.as_str() {
        "avif" => {
            let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
            let mut data = Vec::new();
            use std::io::Read;
            file.read_to_end(&mut data).map_err(|e| e.to_string())?;

            let img = libavif_image::read(&data).map_err(|e| e.to_string())?;
            let rgba = img.into_rgba8();
            
            use image::GenericImageView;
            let (width, height) = rgba.dimensions();
            Ok((width, height, rgba.into_raw()))
        }
        "heic" | "heif" => {
            let path_str = path.to_str().ok_or("Invalid path")?;
            let ctx = libheif_rs::HeifContext::read_from_file(path_str).map_err(|e| e.to_string())?;
            let handle = ctx.primary_image_handle().map_err(|e| e.to_string())?;
            let image = handle.decode(libheif_rs::ColorSpace::Rgb(libheif_rs::RgbChroma::Rgba), libheif_rs::DecodingOptions::new().unwrap())
                .map_err(|e| e.to_string())?;
            let planes = image.planes();
            let plane = planes.interleaved.ok_or_else(|| "No interleaved plane found".to_string())?;
            
            let width = plane.width;
            let height = plane.height;
            let stride = plane.stride;
            let data = plane.data;
            
            let mut rgba = Vec::with_capacity((width * height * 4) as usize);
            if stride == width as usize * 4 {
                rgba.extend_from_slice(data);
            } else {
                for y in 0..height as usize {
                    let start = y * stride;
                    let end = start + width as usize * 4;
                    rgba.extend_from_slice(&data[start..end]);
                }
            }
            Ok((width, height, rgba))
        }
        "cr2" | "nef" | "arw" | "dng" => {
            Err("RAW formats require libraw-rs integration which is scheduled for the next sub-phase.".to_string())
        }
        _ => Err(format!("Unsupported worker format: {}", ext)),
    }
}
