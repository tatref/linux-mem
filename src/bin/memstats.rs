#![allow(non_snake_case)]

// Process groups memory statistics tool
// - Must run as root
// - Don't forget to set a memory limit (-m/--memory-limit) if you read shm pages (-r/--read-shm)
//
//
// TODO:
// - unreadable shm pages
// - better error message for too many open files
// - add tmpfs: shared, cache computation
// - anon / file
// - replace libc by nix?
// - parallelize single pass
// - remove unwraps
// - custom hashset for u64?
//

use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use itertools::Itertools;
use log::warn;
use log::{debug, error, info};
use procfs::{
    prelude::*,
    process::{Pfn, Process},
    PhysicalPageFlags, Shm,
};
use rayon::prelude::*;
use rustc_hash::FxHasher;
use snap::tmpfs::format_units_MiB;
use snap::{
    filters, get_process_info, get_smon_info, groups, LargePages, ProcessInfo, ShmsMetadata,
    SmonInfo,
};
use tabled::Tabled;

use std::{
    collections::{HashMap, HashSet},
    hash::BuildHasherDefault,
    num::NonZeroUsize,
    sync::{Arc, Mutex},
};

use groups::{
    ProcessSplitter, ProcessSplitterCustomFilter, ProcessSplitterEnvVariable, ProcessSplitterUid,
};

use snap::process_tree::ProcessTree;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let global_chrono = std::time::Instant::now();

    const AFTER_HELP: &str = r"Homepage: https://github.com/tatref/linux-mem

/!\ Always set a memory limit /!\

Default limits:
    - memory: available memory / 2
    - threads: available CPU threads / 2

Available filters:
    - true()
    - false()
    - or(..)
    - and(..)
    - not(..)
    - uid(<uid>)
    - descendants(<pid>)
    - pid(<pid>)
    - comm(<comm>)
    - env_k(<env key>)
    - env_kv(<env key,env value>)
Limitation:
    - ALL filters require trailing parenthesis, even true/false
    - Spaces are not allowed before/after commas
    - Characters can't be escaped at the moment
Examples:
    - All `cat` processes: comm(cat)
    - All processes for user 1000: uid(1000)
    - All processes that have a `DISPLAY` env variable (whatever its value is): env_k(DISPLAY)
    - All processes that have a `SHELL` env variable with value `/bin/bash`: env_kv(SHELL,/bin/bash)
    - All non-root processes that have a `DISPLAY` env variable: and(not(uid(0)),env_k(DISPLAY))
    ";

    #[derive(Parser, Debug)]
    #[command(author, version = option_env!("VERSION").unwrap_or("0.1"), about, long_about = None, after_help = AFTER_HELP)]
    struct Cli {
        #[arg(long, hide(true))]
        scan_kpageflags: bool,

        #[arg(short, long)]
        mem_limit: Option<u64>,

        #[arg(short, long)]
        threads: Option<usize>,

        #[arg(short, long)]
        global_stats: bool,

        #[arg(
            short,
            long,
            help = "Filter to scan only a subset of processes. See below for syntax"
        )]
        filter: Option<String>,

        #[arg(short, long, help = "/proc")]
        procfs_root: Option<String>,

        #[arg(
            short,
            long,
            help = "List processes that will be scanned, useful to validate filters"
        )]
        list_processes: bool,

        #[arg(short, long, action = clap::ArgAction::Set, default_value_t = false, help = "Force read PFN for shm, even if shm is in swap")]
        force_read_shm: bool,

        #[command(subcommand)]
        commands: Commands,
    }

    #[derive(Debug, Subcommand)]
    enum Commands {
        #[command(hide = true)]
        GetDbInfo {
            #[arg(long, required = true)]
            pid: i32,
        },
        /// Single threaded process scan, can't do multiple groups, but memory efficient
        Single,
        /// Multi threaded process scan, multiple groups, memory hungry
        Groups {
            #[arg(short = 'e', long)]
            split_env: Option<String>,

            #[arg(short = 'u', long)]
            split_uid: bool,

            #[arg(short = 'p', long, action = clap::ArgAction::Append)]
            split_pids: Vec<i32>,

            #[arg(
                short = 'c',
                long,
                help = "Comma separated list of filters, evaluated in order. Can be repeated to create multiple reports"
            )]
            split_custom: Vec<String>,
        },
    }

    let kernel = procfs::KernelVersion::current().expect("Can't get kernel version");
    if kernel < procfs::KernelVersion::new(2, 6, 32) {
        warn!("Untested kernel version {:?}", kernel);
    }

    let cli = Cli::parse();

    if let Commands::GetDbInfo { pid } = cli.commands {
        // oracle shouldn't run as root
        assert_ne!(uzers::get_effective_uid(), 0);

        // subprogram to connect to instance and print sga size
        // We must have the correct context (user, env vars) to connect to database
        let (sga_size, processes, pga_size, large_pages) = snap::get_db_info().unwrap();

        let sid = std::env::var_os("ORACLE_SID").expect("Missing ORACLE_SID");

        let smon_info: SmonInfo = SmonInfo {
            pid,
            sid: sid.clone(),
            sga_size,
            large_pages,
            processes,
            pga_size,
        };
        let out = serde_json::to_string(&smon_info)
            .expect(&format!("Can't serialize SmonInfo for {sid:?}"));
        println!("{out}");

        // print value, can't use logger here
        // parent will grab that value in `get_smon_info`
        //println!("{sga_size} {processes} {pga_size} {large_pages}");
        std::process::exit(0);
    }
    // can't print anything before that line
    // -------------------------------------

    let mem_limit = if let Some(m) = cli.mem_limit {
        m
    } else {
        let meminfo = procfs::Meminfo::current().unwrap();
        let available = meminfo.mem_available.unwrap_or_else(|| {
            // estimate available memory if field does not exist
            // Target is kernel 2.6.32 if possible
            // https://access.redhat.com/solutions/5928841

            let mut available = meminfo.mem_free;
            available += (meminfo.active_file.unwrap() + meminfo.inactive_file.unwrap()) / 2;
            available += meminfo.s_reclaimable.unwrap();

            available
        });

        // 0.5 * available memory
        available / 1024 / 1024 / 2
    };
    debug!("Memory limit: {mem_limit} MiB");
    let threads = if let Some(t) = cli.threads {
        t
    } else {
        std::thread::available_parallelism()
            .unwrap_or(NonZeroUsize::new(1).unwrap())
            .get()
            / 2
    };
    rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .build_global()
        .unwrap();

    debug!("Using {threads} threads");
    debug!("");

    // Main program starts here
    if uzers::get_effective_uid() != 0 {
        error!("Run as root");
        std::process::exit(1);
    }

    snap::tmpfs::display_tmpfs();

    println!("Scanning /proc/kpageflags...");
    let mut kpageflags = procfs::KPageFlags::new().expect("Can't open /proc/kpageflags");
    let all_physical_pages: HashMap<Pfn, PhysicalPageFlags> = procfs::iomem()
        .expect("Can't read iomem")
        .iter()
        .filter_map(|(_indent, map)| {
            if map.name == "System RAM" {
                Some(map)
            } else {
                None
            }
        })
        .map(|map| {
            let (start, end) = map.get_range().get();

            //let counts = kpagecount
            //    .get_count_in_range(start, end)
            //    .expect("Can't read /proc/kpagecount");
            let flags = kpageflags
                .get_range_info(start, end)
                .expect("Can't read /proc/kpagecount");
            let pfns: Vec<Pfn> = (start.0..end.0).map(Pfn).collect();

            use itertools::izip;
            let v: Vec<(Pfn, PhysicalPageFlags)> = izip!(pfns, flags).collect();

            v
        })
        .flatten()
        .collect();
    println!();

    // find smons processes, and for each spawn a new process in the correct context to get database info
    println!("Scanning Oracle instances...");
    let mut instances: Vec<SmonInfo> = snap::find_smons()
        .iter()
        .filter_map(|(pid, uid, sid, home)| {
            debug!("Getting DB info for pid={pid}, uid={uid}, sid={sid:?}, home={home:?}");
            let smon_info = get_smon_info(*pid, *uid, sid.as_os_str(), home.as_os_str());

            match smon_info {
                Ok(x) => Some(x),
                Err(e) => {
                    warn!("Can't get DB info for {sid:?}: {e:?}");
                    None
                }
            }
        })
        .collect();

    instances.sort_by(|a, b| a.sga_size.cmp(&b.sga_size).reverse());

    #[derive(Tabled)]
    struct InstanceDisplayRow {
        sid: String,
        #[tabled(display_with = "format_units_MiB")]
        sga: u64,
        #[tabled(display_with = "format_units_MiB")]
        pga: u64,
        processes: u64,
        large_pages: LargePages,
    }

    if !instances.is_empty() {
        println!("Oracle instances (MiB):");

        let display_info: Vec<InstanceDisplayRow> = instances
            .iter()
            .map(|instance| InstanceDisplayRow {
                sid: instance.sid.to_string_lossy().to_string(),
                sga: instance.sga_size,
                pga: instance.pga_size,
                processes: instance.processes,
                large_pages: instance.large_pages,
            })
            .collect();

        let mut table = tabled::Table::new(&display_info);
        table.with(tabled::settings::Style::sharp());
        println!("{}", table.to_string());

        println!();
    } else {
        println!("Can't locate any Oracle instance");
        println!();
    }

    println!("Scanning shm...");
    // TODO: remove double read
    for shm in procfs::SharedMemorySegments::current()
        .expect("Can't read /dev/sysvipc/shm")
        .0
    {
        // dummy scan shm so rss is in sync with number of pages
        let _x = snap::shm2pfns(&all_physical_pages, &shm, cli.force_read_shm).unwrap();
    }

    let mut shms_metadata: ShmsMetadata = HashMap::default();
    for shm in procfs::SharedMemorySegments::current()
        .expect("Can't read /dev/sysvipc/shm")
        .0
    {
        let x = match snap::shm2pfns(&all_physical_pages, &shm, cli.force_read_shm) {
            Ok(x) => x,
            Err(e) => {
                warn!("Can't read shm {} {e:?}", shm.key);
                continue;
            }
        };
        shms_metadata.insert(shm, x);
    }

    if !shms_metadata.is_empty() {
        let mut shms: Vec<Shm> = shms_metadata.keys().copied().collect();
        shms.sort_by(|a, b| a.size.cmp(&b.size).reverse());

        #[derive(Tabled)]
        struct ShmDisplayRow {
            key: i32,
            shmid: u64,
            #[tabled(display_with = "format_units_MiB")]
            size: u64,
            #[tabled(display_with = "format_units_MiB")]
            rss: u64,
            pages_4k: String,
            pages_2M: String,
            #[tabled(display_with = "format_units_MiB")]
            swap: u64,
            #[tabled(rename = "used %")]
            used: f32,
            sid: String,
        }

        println!("Sysvipc shm:");
        let mut shm_display = Vec::new();
        for shm in &shms {
            let mut sid_list = Vec::new();
            for instance in &instances {
                // we associate each shm with an sid by looking for smon processes
                let Ok(process) = Process::new(instance.pid) else {
                    continue;
                };
                let Ok(process_info) = get_process_info(process, &shms_metadata) else {
                    continue;
                };

                if process_info.referenced_shms.contains(shm) {
                    sid_list.push(instance.sid.to_string_lossy().to_string());
                }
            }

            // TODO: remove unwrap
            let (pages_4k, pages_2M) = match shms_metadata.get(shm).unwrap() {
                Some((_pfns, _swap_pages, pages_4k, pages_2M)) => {
                    (format!("{}", pages_4k), format!("{}", pages_2M))
                }
                None => ("-".into(), "-".into()),
            };

            let shm_display_row = ShmDisplayRow {
                key: shm.key,
                shmid: shm.shmid,
                size: shm.size,
                rss: shm.rss,
                pages_2M,
                pages_4k,
                swap: shm.swap,
                // USED% can be >100% if size is not aligned with the underling pages: in that case, size < rss+swap
                used: (shm.rss + shm.swap) as f32 / shm.size as f32 * 100.,
                sid: sid_list.join(" "),
            };
            shm_display.push(shm_display_row);
        }

        let mut table = tabled::Table::new(&shm_display);
        table.with(tabled::settings::Style::sharp());

        println!("{table}");

        println!();
    } else {
        println!("Can't locate any shared memory segment");
        println!();
    }

    // probably incorrect?
    // size of kernel structures
    //let current_kernel = procfs::sys::kernel::Version::current().unwrap();
    //let (fd_size, task_size) =
    //    snap::get_kernel_datastructure_size(current_kernel).expect("Unknown kernel");

    //let mut kpagecount = procfs::KPageCount::new().expect("Can't open /proc/kpagecount");

    // processes are scanned once and reused to get a more consistent view
    let mut kernel_processes_count = 0;
    let procfs_root = cli
        .procfs_root
        .map(|s| s.to_string())
        .unwrap_or("/proc".to_string());
    let all_processes: Vec<Process> = procfs::process::all_processes_with_root(procfs_root)
        .unwrap()
        .filter_map(|p| match p {
            Ok(p) => Some(p),
            Err(e) => match e {
                procfs::ProcError::NotFound(_) => None,
                x => {
                    log::error!("Can't read process {x:?}");
                    std::process::exit(1);
                }
            },
        })
        .collect();
    let all_processes_count = all_processes.len();
    info!("Total processes {all_processes_count}");
    let tree = ProcessTree::new(&all_processes);

    // exclude kernel procs
    let processes: Vec<Process> = all_processes
        .into_iter()
        .filter_map(|proc| {
            if proc.cmdline().ok()?.is_empty() {
                kernel_processes_count += 1;
                None
            } else {
                Some(proc)
            }
        })
        .collect();
    info!("Excluded {} kernel processes", kernel_processes_count);

    let processes: Vec<Process> = if let Some(filter) = cli.filter {
        let (f, ate) = filters::parse(&filter).unwrap();
        if filter.chars().count() != ate {
            warn!("Ate {ate}, but filter is {} chars", filter.chars().count());
        }

        let processes: Vec<Process> = processes.into_iter().filter(|p| f.eval(p, &tree)).collect();
        let processes_count = processes.len();

        if processes_count == 0 {
            warn!("Filter excluded all processes");
            warn!("filter: {filter:?}");
            return;
        }

        info!(
            "Filter excluded {} processes, {} processes remaining",
            all_processes_count - processes_count,
            processes_count
        );

        processes
    } else {
        processes
    };
    //println!("");

    if cli.list_processes {
        println!("       uid        pid comm");
        println!("==========================");
        for (uid, pid, comm) in processes
            .iter()
            .inspect(|p| {
                debug!("uid: {:?}", p.uid());
                debug!("stat: {:?}", p.stat());
                p.uid().unwrap();
            })
            .filter_map(|p| Some((p.uid().ok()?, p.pid, p.stat().ok()?.comm)))
        {
            println!("{uid:>10} {pid:>10} {comm}");
        }
        println!();
    }

    let my_pid = std::process::id();
    let my_process = procfs::process::Process::new(my_pid as i32).unwrap();

    match cli.commands {
        Commands::GetDbInfo { .. } => unreachable!(),
        Commands::Single => {
            scan_single(
                my_process,
                global_chrono,
                mem_limit,
                processes,
                &tree,
                &shms_metadata,
            );
        }
        Commands::Groups {
            split_env,
            split_uid,
            split_pids,
            mut split_custom,
        } => {
            split_custom.reverse();

            scan_groups(
                my_process,
                global_chrono,
                mem_limit,
                processes,
                &tree,
                &shms_metadata,
                split_env,
                split_uid,
                split_pids,
                split_custom,
            );
        }
    }

    fn scan_single(
        my_process: Process,
        global_chrono: std::time::Instant,
        mem_limit: u64,
        processes: Vec<Process>,
        _tree: &ProcessTree,
        shms_metadata: &ShmsMetadata,
    ) {
        let processes_count = processes.len();
        let single_chrono = std::time::Instant::now();
        let hit_memory_limit = Arc::new(Mutex::new(false));

        let mut mem_pages: HashSet<Pfn, BuildHasherDefault<FxHasher>> = HashSet::default();
        let mut swap_pages: HashSet<(u64, u64), BuildHasherDefault<FxHasher>> = HashSet::default();
        let mut referenced_shm: HashSet<Shm> = HashSet::new();
        let mut scanned_processes = 0;

        #[allow(unused_variables)]
        let mut vanished = 0;
        let pb = ProgressBar::new(processes_count as u64);
        pb.set_style(ProgressStyle::with_template("{msg} {wide_bar} {pos}/{len}").unwrap());
        for process in processes {
            let my_rss = my_process.status().unwrap().vmrss.unwrap() / 1024;
            pb.set_message(format!("{my_rss}/{mem_limit} MiB"));

            if my_rss > mem_limit {
                let mut guard = hit_memory_limit.lock().unwrap();
                if !*guard {
                    warn!(
                        "Hit memory limit ({} MiB), try increasing limit or filtering processes",
                        mem_limit
                    );
                    *guard = true;
                }
                break;
            }
            let process_info = match get_process_info(process, shms_metadata) {
                Ok(info) => info,
                Err(_) => {
                    vanished += 1;
                    continue;
                }
            };
            scanned_processes += 1;

            mem_pages.par_extend(&process_info.pfns);
            swap_pages.par_extend(&process_info.swap_pages);
            referenced_shm.extend(process_info.referenced_shms);
            pb.inc(1);
        }
        pb.finish_and_clear();

        let rss = mem_pages.len() as u64 * procfs::page_size() / 1024 / 1024;
        let swap = swap_pages.len() as u64 * procfs::page_size() / 1024 / 1024;
        let shm_mem: u64 = referenced_shm.iter().map(|shm| shm.rss).sum::<u64>() / 1024 / 1024;
        let shm_swap: u64 = referenced_shm.iter().map(|shm| shm.swap).sum::<u64>() / 1024 / 1024;

        println!(
            "{} processes scanned in {:?}",
            scanned_processes,
            single_chrono.elapsed()
        );
        info!("");
        info!("Statistics:");
        info!("mem RSS: {rss}");
        info!("swap RSS: {swap}");
        info!("shm mem: {shm_mem}");
        info!("shm swap: {shm_swap}");

        finalize(hit_memory_limit, mem_limit, &my_process, global_chrono);
    }

    fn scan_groups(
        my_process: Process,
        global_chrono: std::time::Instant,
        mem_limit: u64,
        processes: Vec<Process>,
        tree: &ProcessTree,
        shms_metadata: &ShmsMetadata,
        split_env: Option<String>,
        split_uid: bool,
        split_pids: Vec<i32>,
        mut split_custom: Vec<String>,
    ) {
        let processes_count = processes.len();
        let hit_memory_limit = Arc::new(Mutex::new(false));
        let chrono = std::time::Instant::now();
        println!("\nScanning {processes_count} processes");
        let pb = ProgressBar::new(processes_count as u64);
        pb.set_style(ProgressStyle::with_template("{msg} {wide_bar} {pos}/{len}").unwrap());
        let processes_info: Vec<ProcessInfo> = processes
            .into_par_iter()
            //.progress_count(processes_count as u64)
            .filter_map(|proc| {
                let my_rss = my_process.status().unwrap().vmrss.unwrap() / 1024;
                pb.set_message(format!("{my_rss}/{mem_limit} MiB"));

                if my_rss > mem_limit {
                    let mut guard = hit_memory_limit.lock().unwrap();
                    if !*guard {
                        warn!("Hit memory limit ({} MiB), try increasing limit or filtering processes", mem_limit);
                        *guard = true;
                    }
                    return None;
                }

                if proc.pid != my_process.pid {
                    let info = get_process_info(proc, shms_metadata).ok()?;
                    pb.inc(1);
                    Some(info)
                } else {
                    pb.inc(1);
                    None
                }
            })
            .collect();
        pb.finish_and_clear();

        let vanished_processes_count = processes_count - processes_info.len();

        println!(
            "Scanned {} processes in {:?}",
            processes_info.len(),
            chrono.elapsed()
        );
        info!("{} processe(s) vanished", vanished_processes_count);
        info!("");

        {
            // scan missing SHM
            let missing_shms: Vec<_> = processes_info
                .iter()
                .filter_map(|process_info| {
                    if process_info.unknown_shm.is_empty() {
                        None
                    } else {
                        Some((process_info.process.pid, process_info.unknown_shm.clone()))
                    }
                })
                .collect();
            let mut more_pids_and_shm = HashMap::new();
            for (pid, shms) in &missing_shms {
                for shm in shms {
                    more_pids_and_shm.entry(shm).or_insert(Vec::new()).push(pid);
                }
            }

            //dbg!(&more_pids_and_shm);

            for (_shm, pids) in more_pids_and_shm.iter_mut() {
                for _p in pids {
                    // TODO
                    //let if Ok(shm_metadata) = scan_pid_shm(p, shm) {
                    //  shm.append(shm_metadata);
                    //  for pid in &pids {
                    //      for process_info in processes_info.iter_mut() {
                    //          if process_info.process.pid == pid {
                    //              process_info.referenced_shm.insert(shm_metadata);
                    //          }
                    //      }
                    //  }
                    //  break;
                    //}
                    //else {
                    //};
                }
            }
        }

        println!();
        let processes_info: Vec<ProcessInfo> = if split_uid {
            let mut splitter = ProcessSplitterUid::new();
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            splitter.collect_processes()
        } else {
            processes_info
        };

        let processes_info: Vec<ProcessInfo> = if let Some(var) = split_env {
            let mut splitter = ProcessSplitterEnvVariable::new(var);
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            splitter.collect_processes()
        } else {
            processes_info
        };

        let processes_info = if !split_pids.is_empty() {
            // Waiting for deletion
            //let mut splitter = ProcessSplitterPids::new(&split_pids);

            // pid(1),pid(2),pid(3),...
            let expr = match split_pids.len() {
                1 => format!("pid({})", split_pids.first().unwrap()),
                _ => {
                    let custom_pids = split_pids
                        .iter()
                        .map(|pid| format!("pid({})", pid))
                        .join(",");
                    format!("or({})", custom_pids)
                }
            };

            let mut splitter = ProcessSplitterCustomFilter::new(&expr).unwrap();
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            splitter.collect_processes()
        } else {
            processes_info
        };

        let mut processes_info = processes_info;
        while let Some(filter) = split_custom.pop() {
            let mut splitter = ProcessSplitterCustomFilter::new(&filter).unwrap();
            splitter.split(tree, shms_metadata, processes_info);
            splitter.display(shms_metadata);
            processes_info = splitter.collect_processes();
        }

        finalize(hit_memory_limit, mem_limit, &my_process, global_chrono);
    }

    fn finalize(
        hit_memory_limit: Arc<Mutex<bool>>,
        mem_limit: u64,
        _my_process: &Process,
        global_chrono: std::time::Instant,
    ) {
        if *hit_memory_limit.lock().unwrap() {
            warn!(
                "Hit memory limit ({} MiB), try increasing limit or filtering processes",
                mem_limit
            )
        }

        let global_elapsed = global_chrono.elapsed();

        info!("");
        info!("global_elapsed = {global_elapsed:?}");
    }
}
