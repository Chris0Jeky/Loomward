#[cfg(windows)]
mod windows;

fn main() {
    #[cfg(windows)]
    if let Err(error) = windows::run() {
        eprintln!("loomward-lab: {error}");
        std::process::exit(1);
    }
    #[cfg(not(windows))]
    {
        eprintln!("loomward-lab generation and measurement require Windows NTFS; portable plan/decoder tests are available via cargo test");
        std::process::exit(1);
    }
}
