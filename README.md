# Linux memory tools

A toolbox to inspect Linux memory

https://github.com/tatref/linux-mem


# Big tools
## [procinfo](src/bin/procinfo.rs)

Memory map details for single process. List virtual memory, physical pages, physical flags...

Information is grabbed from `/proc/<pid>/smaps`, `/proc/<pid>/pagemap`, `/proc/kpageflags`

Usage: `procinfo <pid...>`

```
# procinfo 12345
0x00007ff437847000-0x00007ff437849000 MMPermissions(NONE | READ | WRITE | PRIVATE) 0 Anonymous
PFN=0x0000159f83 MemoryPageFlags(SOFT_DIRTY | PRESENT | 0x159f83) / Some(PhysicalPageFlags(UPTODATE | LRU | MMAP | ANON | SWAPBACKED))
PFN=0x000010a5cb MemoryPageFlags(SOFT_DIRTY | PRESENT | 0x10a5cb) / Some(PhysicalPageFlags(UPTODATE | LRU | MMAP | ANON | SWAPBACKED))
stats: VSZ=8 kiB, RSS=8 kiB, SWAP=0 kiB
0x00007ff43784d000-0x00007ff437854000 MMPermissions(NONE | READ | SHARED) 160259 Path("/usr/lib64/gconv/gconv-modules.cache")
PFN=0x0000109d63 MemoryPageFlags(SOFT_DIRTY | FILE | PRESENT | 0x109d63) / Some(PhysicalPageFlags(REFERENCED | UPTODATE | LRU | ACTIVE | MMAP))
PFN=0x0000109d5b MemoryPageFlags(SOFT_DIRTY | FILE | PRESENT | 0x109d5b) / Some(PhysicalPageFlags(REFERENCED | UPTODATE | LRU | ACTIVE | MMAP))
```

## [snap.py](proc_snap/README.md)

/proc snapshot tool

## [memstats](src/bin/memstats.rs)

Memory usage for groups of processes. RSS and USS are computed from physical pages allocation, this is not a simple sum of each process.

Groups can be created by user, by environment variable, by user provided PIDs list, or by custom filters

Example invocation:

```
$ sudo ./memstats groups --split-uid --split-env ORACLE_SID
Scanning tmpfs...
┌────────────────┬─────────┬─────────┐
│ mount_point    │ fs_size │ fs_used │
├────────────────┼─────────┼─────────┤
│ /dev/shm       │ 0.02 MB │ 0.02 MB │
│ /run           │ 9.01 MB │ 9.01 MB │
│ /sys/fs/cgroup │ 0 MB    │ 0 MB    │
│ /run/user/0    │ 0 MB    │ 0 MB    │
└────────────────┴─────────┴─────────┘

Scanning /proc/kpageflags...

Scanning Oracle instances...
Oracle instances (MiB):
SID                  SGA         PGA  PROCESSES  LARGE_PAGES
============================================================
orcl                24512        327         88         TRUE

Scanning shm...
Shared memory segments (MiB):
         key           id       Size        RSS         4k/2M        SWAP   USED%        SID
============================================================================================
           0            2      22528      22528    5767168/0            0  100.00       orcl
           0            1       1984       1984          0/992          0  100.00       orcl
           0            3         54         54      14015/0            0  100.00       orcl
           0            0         10         10          0/5            0  100.00       orcl
  1966876864            4          0          0         17/0            0  100.00       orcl


Scanning 117 processes
Scanned 116 processes in 98.467317ms
┌────────────┬───────┬─────────────┬─────────────┬──────────┬──────────┬─────────────┬──────────┐
│ group_name │ procs │ mem_rss     │ mem_uss     │ swap_rss │ swap_uss │ shm_mem     │ shm_swap │
├────────────┼───────┼─────────────┼─────────────┼──────────┼──────────┼─────────────┼──────────┤
│ oracle     │ 80    │ 26399.67 MB │ 26394.73 MB │ 0 MB     │ 0 MB     │ 25770.66 MB │ 0 MB     │
│ root       │ 31    │ 123.46 MB   │ 111.70 MB   │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
│ polkitd    │ 1     │ 13.83 MB    │ 6.98 MB     │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
│ postfix    │ 2     │ 8.09 MB     │ 2.90 MB     │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
│ dbus       │ 1     │ 4.83 MB     │ 1.04 MB     │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
│ rpc        │ 1     │ 3.59 MB     │ 0.73 MB     │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
└────────────┴───────┴─────────────┴─────────────┴──────────┴──────────┴─────────────┴──────────┘

┌──────────────┬───────┬─────────────┬─────────────┬──────────┬──────────┬─────────────┬──────────┐
│ group_name   │ procs │ mem_rss     │ mem_uss     │ swap_rss │ swap_uss │ shm_mem     │ shm_swap │
├──────────────┼───────┼─────────────┼─────────────┼──────────┼──────────┼─────────────┼──────────┤
│ Some("orcl") │ 79    │ 26396.70 MB │ 26393.29 MB │ 0 MB     │ 0 MB     │ 25770.66 MB │ 0 MB     │
│ None         │ 37    │ 136.56 MB   │ 133.16 MB   │ 0 MB     │ 0 MB     │ 0 MB        │ 0 MB     │
└──────────────┴───────┴─────────────┴─────────────┴──────────┴──────────┴─────────────┴──────────┘

```

You can repeat `--split-custom` multiple times

Set colors with `COLORS` env variable. Possible values:
* no / nocolors
* magma
* turbo
* spectral
* viridis
* inferno
* plasma
* rainbow
* sinebow

### How it works
1. list all processes
1. exlude kernel processes, exclude processes not matching filter
1. For each process, compute the set of pages referenced (via `/proc/<pid>/smaps` and `/proc/<pid>/pagemap`)
1. For each process group, compute the union of sets
1. For each group, compute the difference between this groups' set and others', this gives the group USS (memory only referenced by processes in this group). RSS is memory referenced by this group that may also be referenced by processes in other groups

![Memory groups Venn diagram RSS USS](./assets/Process_groups_RSS_USS.png)

### Building

Multiple hash functions can be used. Seems that `fxhash` is the fastest

features :
* `--features fxhash` (default)
* `--features ahash`
* `--features fnv`
* `--features metrohash`
* `--features std`


Require a fork of procfs until PR 254 is merged (https://github.com/eminence/procfs/pull/254)

To compile for old glibc, install [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild)

Via zigbuild
```
arch=x86-64-v2
RUSTFLAGS="-C target-cpu=$arch" cargo zigbuild --release --bin memstats --target x86_64-unknown-linux-gnu.2.12
```

Or if you don't need a portable binary
```
cargo build --release --bin memstats
```

## [processes2png](src/bin/processes2png.rs)

Visual map of processes memory

For details, see [my blog post](https://tatref.github.io/blog/2023-visual-linux-memory-compact/)


Effect of memory compaction:

![gif](https://tatref.github.io/blog/2023-visual-linux-memory-compact/out.gif)


# Small tools
## [shmat](src/bin/shmat.rs)

Attach shared memory segments to current process

## [shmem](src/bin/shmem.rs)

Shared memory tool

## [connections](oracle-tools/src/bin/connections.rs)

Establish lots of connections to Oracle database

## [find_instances](oracle-tools/src/bin/find_instances.rs)

Find Oracle database instances, connect to DB and run some request. Env variables (SID, lib...) and user are found automatically.
