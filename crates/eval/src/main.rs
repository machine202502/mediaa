fn main() {
    if let Err(error) = mediaa_eval::main_args(std::env::args().skip(1)) {
        eprintln!("error: {error:#}");
        std::process::exit(1);
    }
}
