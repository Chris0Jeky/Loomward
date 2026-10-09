use loomward_windows::{fixtures, volumes};
use std::{env, io, path::Path};

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [command, flag] if command == "volumes" && flag == "--json" => println!("{}", serde_json::to_string_pretty(&volumes::enumerate()?)?),
        [command, flag, root] if flag == "--root" && command == "create" => println!("{}", serde_json::to_string_pretty(&fixtures::create(Path::new(root))?)?),
        [command, flag, root] if flag == "--root" && command == "destroy" => fixtures::destroy(Path::new(root))?,
        _ => return Err(io::Error::new(io::ErrorKind::InvalidInput, "usage: loomward-fixtures create|destroy --root <dir> | volumes --json (fixture root must be below G:\\loomward-lab\\fixtures)").into()),
    }
    Ok(())
}
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}
