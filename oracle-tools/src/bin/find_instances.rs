// List Oracle database instances running on the server
// Start as root

use oracle::{Connector, Privilege};
use std::env;
use std::ffi::OsString;
use std::os::unix::process::CommandExt;
use std::process::Command;

/// Connect to DB using OS auth and env vars
/// return size of SGA
fn get_sga_size() -> u64 {
    let conn = Connector::new("", "", "")
        .external_auth(true)
        .privilege(Privilege::Sysdba)
        .connect()
        .unwrap();
    let sql = "select sum(value) from v$sga where name in ('Variable Size', 'Database Buffers')";
    let sga_size = conn.query_row_as::<u64>(sql, &[]).expect("query failed");

    sga_size
}

/// Find smons processes
/// For each, return (pid, uid, ORACLE_SID, ORACLE_HOME)
fn find_smons() -> Vec<(i32, u32, OsString, OsString)> {
    let smons: Vec<_> = procfs::process::all_processes()
        .unwrap()
        .filter(|proc| {
            let cmdline = proc.as_ref().unwrap().cmdline().unwrap();
            if cmdline.len() == 1 && cmdline[0].starts_with("ora_smon_") {
                true
            } else {
                false
            }
        })
        .map(|p| p.unwrap())
        .collect();

    smons
        .iter()
        .map(|smon| {
            let pid = smon.pid;
            let uid = smon.uid().unwrap();
            let environ = smon.environ().unwrap();
            let sid = environ
                .get(&OsString::from("ORACLE_SID"))
                .unwrap()
                .to_os_string();
            let home = environ
                .get(&OsString::from("ORACLE_HOME"))
                .unwrap()
                .to_os_string();

            (pid, uid, sid, home)
        })
        .collect()
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::var("ORACLE_SID").is_err() {
        if users::get_effective_uid() != 0 {
            panic!("Run as root");
        }
        // first run
        // find smons processes, and for each spawn a new process in the correct context

        let instances = find_smons();
        for (_pid, uid, sid, home) in instances.iter() {
            let myself = env::args().nth(0).unwrap();

            let mut lib = home.clone();
            lib.push("/lib");

            let output = Command::new(myself)
                .env("LD_LIBRARY_PATH", lib)
                .env("ORACLE_SID", sid)
                .env("ORACLE_HOME", home)
                .uid(*uid)
                .output()
                .expect("failed to execute process");

            if !output.status.success() {
                println!("Can't get info for {sid:?}: {:?}", output);
                continue;
            }

            let stdout = match String::from_utf8(output.stdout.clone()) {
                Ok(s) => s,
                Err(_) => {
                    println!("Can't read output for {sid:?}: {:?}", output);
                    continue;
                }
            };

            let sga_size: usize = stdout.trim().parse().unwrap();

            println!("sid={sid:?} sga={sga_size}");
        }
    } else {
        // We should have the correct context (user, env vars) to connect to database

        let sga_size = get_sga_size();
        println!("{sga_size}");
    }

    Ok(())
}
