mod app;
mod scan;
mod treemap;

use std::ffi::OsString;
use std::path::PathBuf;

use color_eyre::eyre::{self, WrapErr};

fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    let path = match cli()? {
        Cli::Help => {
            println!(
                "Usage: foo [PATH]\n\nScan PATH and show disk usage. PATH defaults to the current directory."
            );
            return Ok(());
        }
        Cli::Path(path) => path,
    };

    eprintln!("Scanning {}…", path.display());
    let started = std::time::Instant::now();
    let scan = scan::scan(&path).wrap_err_with(|| format!("failed to scan {}", path.display()))?;
    let elapsed = started.elapsed();
    let unread = if scan.errors == 0 {
        String::new()
    } else {
        format!(" ({} unreadable)", scan.errors)
    };
    eprintln!(
        "Scanned {} — {} in {:.1}s{unread}",
        scan.root_path.display(),
        scan::format_size(scan.root.size),
        elapsed.as_secs_f64(),
    );

    let mut app = app::App::new(scan);
    ratatui::run(|terminal| app.run(terminal))?;
    Ok(())
}

enum Cli {
    Help,
    Path(PathBuf),
}

fn cli() -> color_eyre::Result<Cli> {
    let mut args = std::env::args_os().skip(1);
    let Some(arg) = args.next() else {
        return Ok(Cli::Path(std::env::current_dir()?));
    };
    if args.next().is_some() {
        eyre::bail!("expected a single path");
    }
    if is_help(&arg) {
        Ok(Cli::Help)
    } else {
        Ok(Cli::Path(PathBuf::from(arg)))
    }
}

fn is_help(arg: &OsString) -> bool {
    arg == "--help" || arg == "-h"
}
