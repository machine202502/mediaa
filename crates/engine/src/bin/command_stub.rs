use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut index = 0;
    let mut code = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--code" => {
                code = args
                    .get(index + 1)
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(1);
                index += 2;
            }
            "--stderr-file" => {
                let path = args.get(index + 1).cloned().unwrap_or_default();
                eprint!("{}", std::fs::read_to_string(path).unwrap_or_default());
                index += 2;
            }
            "--stdout-file" => {
                let path = args.get(index + 1).cloned().unwrap_or_default();
                print!("{}", std::fs::read_to_string(path).unwrap_or_default());
                index += 2;
            }
            "--hang" => loop {
                std::thread::sleep(Duration::from_secs(30));
            },
            _ => index += 1,
        }
    }
    std::process::exit(code);
}
