mod ipc;
mod lifecycle;
mod settings;
mod tray;

use anyhow::{bail, Result};
use std::{env, process};

fn main() {
    if let Err(error) = run() {
        eprintln!("Unsloth Studio Desktop: {error:#}");
        process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<_> = env::args_os().skip(1).collect();
    match args.as_slice() {
        [arg] if arg == "--help" => {
            println!("Unsloth Studio Desktop\n\nUsage: unsloth-studio-portable.exe [--help|--version]\n\nOpen the native window.");
            Ok(())
        }
        [arg] if arg == "--version" => {
            println!("{}", env!("CARGO_PKG_VERSION"));
            Ok(())
        }
        [] => {
            #[cfg(windows)]
            {
                ipc::run_minimal_ui()
            }
            #[cfg(not(windows))]
            {
                bail!("This launcher supports Windows only")
            }
        }
        [arg, ..] => bail!("unknown or excess argument {arg:?}; use --help"),
    }
}
