//! Disposable AppContainer measurement, never a production teacher runner.
use std::{io, path::PathBuf};

#[cfg(windows)]
#[path = "teacher_sandbox/windows.rs"]
mod windows;

#[derive(Debug, PartialEq)]
enum Role {
    Run(PathBuf),
    Probe(PathBuf),
    Child(PathBuf, bool),
}

fn role(args: &[String]) -> io::Result<Role> {
    match args {
        [flag, path] if flag == "--run" => Ok(Role::Run(path.into())),
        [flag, path] if flag == "--probe" => Ok(Role::Probe(path.into())),
        [flag, path] if flag == "--child" => Ok(Role::Child(path.into(), false)),
        [flag, path, linger] if flag == "--child" && linger == "--linger" => {
            Ok(Role::Child(path.into(), true))
        }
        _ => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "usage: teacher_sandbox --run <out.json> | --probe <private-config.json>",
        )),
    }
}

// CreateProcess receives one command line; quote each trusted argument using Windows rules.
fn command_line(args: &[String]) -> String {
    args.iter()
        .map(|arg| {
            let mut quoted = String::from("\"");
            let mut slashes = 0;
            for c in arg.chars() {
                if c == '\\' {
                    slashes += 1;
                    continue;
                }
                quoted.extend(std::iter::repeat_n(
                    '\\',
                    slashes * if c == '"' { 2 } else { 1 },
                ));
                slashes = 0;
                if c == '"' {
                    quoted.push('\\');
                }
                quoted.push(c);
            }
            quoted.extend(std::iter::repeat_n('\\', slashes * 2));
            quoted.push('"');
            quoted
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn capability_names(internet: bool) -> &'static [&'static str] {
    if internet {
        &["internetClient"]
    } else {
        &[]
    }
}

fn main() {
    let result = role(&std::env::args().skip(1).collect::<Vec<_>>()).and_then(|role| {
        #[cfg(windows)]
        return windows::run(role);
        #[cfg(not(windows))]
        {
            let _ = (role, command_line(&[]), capability_names(false));
            Err::<(), _>(io::Error::other("Windows required; no measurements made"))
        }
    });
    if let Err(error) = result {
        eprintln!("teacher-sandbox: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_roles_only() {
        assert_eq!(
            role(&["--run".into(), "out.json".into()]).unwrap(),
            Role::Run("out.json".into())
        );
        assert_eq!(
            role(&["--probe".into(), "lab.json".into()]).unwrap(),
            Role::Probe("lab.json".into())
        );
        assert!(role(&[]).is_err());
        assert!(role(&["--run".into()]).is_err());
        assert!(role(&["--run".into(), "x".into(), "--internet".into()]).is_err());
    }

    #[test]
    fn capabilities_have_no_ambient_defaults() {
        assert!(capability_names(false).is_empty());
        assert_eq!(capability_names(true), &["internetClient"]);
    }

    #[test]
    fn windows_arguments_keep_quotes_slashes_and_empty_values() {
        assert_eq!(command_line(&["".into(), "a b".into()]), "\"\" \"a b\"");
        assert_eq!(command_line(&["C:\\lab\\".into()]), "\"C:\\lab\\\\\"");
        assert_eq!(command_line(&["a\\\"b".into()]), "\"a\\\\\\\"b\"");
    }
}
