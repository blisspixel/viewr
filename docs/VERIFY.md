# Verifiable Builds & Trust

At `viewr`, privacy is not a promise you have to trust—it is a mathematical property of the code. However, open-source code is only trustworthy if you can guarantee that the release binaries actually match the source code.

This document explains how to perform a **deterministic, verifiable build** of `viewr` so you can independently confirm that the binaries we distribute have not been injected with spyware or network clients.

## The Threat Model
Supply-chain attacks often involve compromising a CI/CD pipeline or a developer's machine so that the published binary contains malicious code not present in the public repository. 

Our defense against this is reproducibility: if you check out the exact commit tag for a release and build it using our locked toolchain, your output should hash to the exact same SHA-256 checksum that we publish.

## 1. Prerequisites
To guarantee determinism, you must use the exact compiler version defined in `rust-toolchain.toml` and the exact dependencies locked in `Cargo.lock`.

```bash
# Clone the repository and checkout a release tag
git clone https://github.com/viewr/viewr.git
cd viewr
git checkout v1.0.0

# Ensure you have the exact Rust toolchain installed
rustup show
```

## 2. Dependency Audit
Before building, you can independently verify that our dependency tree is free of network clients (HTTP, TLS, etc.) by running our `cargo-deny` config.

```bash
cargo install cargo-deny
cargo deny check
```
If this command passes, it mathematically proves that no remote-service client stack has been linked into the dependency graph.

## 3. The Verifiable Build
To build the reproducible binaries (the main app and the isolated decode worker), use the standard release profile. The Cargo configurations are pinned to ensure deterministic optimizer behavior.

```bash
cargo build --release
```

## 4. Comparing Checksums
Once the build is complete, generate the SHA-256 checksums for the binaries:

### Linux / macOS
```bash
sha256sum target/release/viewr
sha256sum target/release/viewr-decode
```

### Windows (PowerShell)
```powershell
Get-FileHash target\release\viewr.exe -Algorithm SHA256
Get-FileHash target\release\viewr-decode.exe -Algorithm SHA256
```

Compare these checksums against the `SHA256SUMS` file published in the GitHub Release assets. If they match exactly, you have mathematically verified that the binaries running on your machine are identical to the auditable open-source code in this repository.

## Notes on OS-Specific Metadata
Some platforms (like macOS) inject timestamped signatures into the final bundle (`.app`), which changes the checksum of the wrapper. Our verifiable checksums apply to the **bare executable binaries** before any OS-specific packaging or ad-hoc signing is applied.
