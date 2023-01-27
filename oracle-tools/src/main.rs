use oracle::{Connection, Error};
use std::env;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();

    dbg!(&args);

    let host = &args[1];
    let service = &args[2];
    let user = &args[3];
    let password = &args[4];
    let max_conn = args[5].parse().unwrap();
    let wait = args[6].parse().unwrap();

    let connections: Vec<Connection> = (0..max_conn)
        .map(|i| Connection::connect(user, password, format!("{}:1521/{}", host, service)).unwrap())
        .collect();

    println!("{} connections", max_conn);

    std::thread::sleep(std::time::Duration::from_secs(wait));

    Ok(())
}
