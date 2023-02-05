use oracle::{Connector, Connection, Privilege};
use std::env;
use std::io::Write;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = env::args().collect();

    dbg!(&args);

    let connect_string = &args[1];
    let user = &args[2];
    let password = &args[3];
    let max_conn = args[4].parse().unwrap();

    let mut con = Connector::new(user, password, connect_string);
    if user == "sys" {
        con.privilege(Privilege::Sysdba);
    }

    let _connections: Vec<Connection> = (0..max_conn)
        .map(|i| {
            if i % 10 == 0 {
                print!("{i}/{max_conn}\r");
                std::io::stdout().lock().flush().unwrap();
            }
            con.connect().unwrap()
        })
        .collect();
    println!("\n{max_conn} connections");

    println!("^C to quit");
    std::thread::sleep(std::time::Duration::from_secs(3600));

    Ok(())
}
