use gsd_multiplexer::{submit, Multiplexer};
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: gsd_multiplexer <command> [args...]");
        eprintln!("Commands:");
        eprintln!("  daemon <root>        - Run as daemon");
        eprintln!("  submit <root> <input> - Submit task and wait for result");
        return ExitCode::FAILURE;
    }

    match args[1].as_str() {
        "daemon" => {
            if args.len() < 3 {
                eprintln!("Usage: gsd_multiplexer daemon <root>");
                return ExitCode::FAILURE;
            }
            let root = &args[2];

            let mut multiplexer = match Multiplexer::new(root) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Failed to create multiplexer: {}", e);
                    return ExitCode::FAILURE;
                }
            };

            if let Err(e) = multiplexer.run() {
                eprintln!("Multiplexer error: {}", e);
                return ExitCode::FAILURE;
            }
        }
        "submit" => {
            if args.len() < 4 {
                eprintln!("Usage: gsd_multiplexer submit <root> <input>");
                return ExitCode::FAILURE;
            }
            let root = &args[2];
            let input = &args[3];

            match submit(root, input) {
                Ok(output) => {
                    print!("{}", output);
                }
                Err(e) => {
                    eprintln!("Submit error: {}", e);
                    return ExitCode::FAILURE;
                }
            }
        }
        _ => {
            eprintln!("Unknown command: {}", args[1]);
            return ExitCode::FAILURE;
        }
    }

    ExitCode::SUCCESS
}
