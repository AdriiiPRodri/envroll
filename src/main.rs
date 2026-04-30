use std::process::ExitCode;

fn main() -> ExitCode {
    match envroll::run() {
        Ok(code) => ExitCode::from(code),
        Err(e) => {
            // Anything that escapes here is an unhandled panic-equivalent;
            // dispatch() is responsible for converting EnvrollError into the
            // correct exit code. If we hit this branch, fall back to 1.
            eprintln!("envroll: {e}");
            ExitCode::from(1)
        }
    }
}
