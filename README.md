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

Memory usage for groups of processes.

![Memory groups Venn diagram RSS USS](./assets/uss_rss.png#1)

Groups can be created by user, by environment variable, by user provided PIDs list, or by custom filters

Example usage:

```
$ sudo ./memstats groups --split-uid --split-env ORACLE_SID
Scanning tmpfs...
┌────────────────┬──────────────┬────────────┐
│ mount_point    │ fs_size      │ fs_used    │
├────────────────┼──────────────┼────────────┤
│ /dev/shm       │ 354435.47 MB │ 1342.18 MB │
│ /run           │ 177218.13 MB │ 13.76 MB   │
│ /sys/fs/cgroup │ 177218.13 MB │ 0 MB       │
│ /run/user/1001 │ 35443.63 MB  │ 0 MB       │
│ /run/user/1010 │ 35443.63 MB  │ 0 MB       │
│ /run/user/0    │ 35443.63 MB  │ 0 MB       │
└────────────────┴──────────────┴────────────┘

Scanning /proc/kpageflags...

Scanning Oracle instances...
Oracle instances (MiB):
┌─────────┬─────────────┬────────────┬───────────┬─────────────┐
│ sid     │ sga         │ pga        │ processes │ large_pages │
├─────────┼─────────────┼────────────┼───────────┼─────────────┤
│ DBA1    │ 68451.04 MB │ 938.67 MB  │ 85        │ Only        │
│ DBB1    │ 21273.51 MB │ 1492.68 MB │ 117       │ Only        │
│ DBC1    │ 10569.65 MB │ 1004.55 MB │ 100       │ Only        │
│ DBD1    │ 4143.97 MB  │ 1008.92 MB │ 97        │ Only        │
│ +ASM1   │ 3170.89 MB  │ 274.43 MB  │ 77        │ False       │
└─────────┴─────────────┴────────────┴───────────┴─────────────┘

Scanning shm...
Shared memory segments (MiB):
┌────────────┬───────────┬─────────────┬─────────────┬──────────┬──────────┬──────┬────────┬─────────┐
│ key        │ shmid     │ size        │ rss         │ pages_4k │ pages_2M │ swap │ used % │ sid     │
├────────────┼───────────┼─────────────┼─────────────┼──────────┼──────────┼──────┼────────┼─────────┤
│ 0          │ 15171592  │ 68451.04 MB │ 68451.04 MB │ 0        │ 32640    │ 0 MB │ 100    │ DBA1    │
│ 0          │ 432308240 │ 21273.51 MB │ 21273.51 MB │ 0        │ 10144    │ 0 MB │ 100    │ DBB1    │
│ 0          │ 7766020   │ 10569.65 MB │ 10569.65 MB │ 0        │ 5040     │ 0 MB │ 100    │ DBC1    │
│ 0          │ 8355852   │ 4143.97 MB  │ 4143.97 MB  │ 0        │ 1976     │ 0 MB │ 100    │ DBD1    │
│ 0          │ 6881281   │ 3204.45 MB  │ 3204.45 MB  │ 782336   │ 0        │ 0 MB │ 100    │ +ASM1   │
│ 0          │ 15204361  │ 255.85 MB   │ 255.85 MB   │ 0        │ 122      │ 0 MB │ 100    │ DBA1    │
│ 0          │ 432341009 │ 188.74 MB   │ 188.74 MB   │ 0        │ 90       │ 0 MB │ 100    │ DBB1    │
│ 0          │ 7798789   │ 155.19 MB   │ 155.19 MB   │ 0        │ 74       │ 0 MB │ 100    │ DBC1    │
│ 0          │ 8388621   │ 142.61 MB   │ 142.61 MB   │ 0        │ 68       │ 0 MB │ 100    │ DBD1    │
│ 0          │ 432275471 │ 14.68 MB    │ 14.68 MB    │ 0        │ 7        │ 0 MB │ 100    │ DBB1    │
│ 0          │ 15138823  │ 14.68 MB    │ 14.68 MB    │ 0        │ 7        │ 0 MB │ 100    │ DBA1    │
│ 0          │ 7733251   │ 14.68 MB    │ 14.68 MB    │ 0        │ 7        │ 0 MB │ 100    │ DBC1    │
│ 0          │ 8323083   │ 10.49 MB    │ 10.49 MB    │ 0        │ 5        │ 0 MB │ 100    │ DBD1    │
│ 0          │ 6848512   │ 8.93 MB     │ 8.93 MB     │ 2181     │ 0        │ 0 MB │ 100    │ +ASM1   │
│ 2082471084 │ 8421390   │ 2.10 MB     │ 2.10 MB     │ 0        │ 1        │ 0 MB │ 100    │ DBD1    │
│ 339729820  │ 432373778 │ 2.10 MB     │ 2.10 MB     │ 0        │ 1        │ 0 MB │ 100    │ DBB1    │
│ 1199684380 │ 15237130  │ 2.10 MB     │ 2.10 MB     │ 0        │ 1        │ 0 MB │ 100    │ DBA1    │
│ -819740076 │ 7831558   │ 2.10 MB     │ 2.10 MB     │ 0        │ 1        │ 0 MB │ 100    │ DBC1    │
│ 1050822864 │ 6914050   │ 0.03 MB     │ 0.03 MB     │ 8        │ 0        │ 0 MB │ 100    │ +ASM1   │
└────────────┴───────────┴─────────────┴─────────────┴──────────┴──────────┴──────┴────────┴─────────┘

Scanning 548 processes
Scanned 544 processes in 1.457584924s

UID
┌────────────┬───────┬────────────┬────────────┬────────────┬───────────┬──────────┬──────────┬──────────────┬──────────┐
│ group_name │ procs │ mem_rss    │ mem_anon   │ mem_uss    │ swap_anon │ swap_rss │ swap_uss │ shm_mem      │ shm_swap │
├────────────┼───────┼────────────┼────────────┼────────────┼───────────┼──────────┼──────────┼──────────────┼──────────┤
│ oracle     │ 372   │ 7172.18 MB │ 5059.76 MB │ 7068.82 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 105243.48 MB │ 0 MB     │
│ grid       │ 91    │ 4354.54 MB │ 3887.93 MB │ 4181.46 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 3213.41 MB   │ 0 MB     │
│ root       │ 53    │ 2817.27 MB │ 2475.34 MB │ 2674.06 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 0 MB         │ 0 MB     │
│ abcd       │ 7     │ 713.86 MB  │ 657.56 MB  │ 706.16 MB  │ 0.01 MB   │ 0.01 MB  │ 0.01 MB  │ 0 MB         │ 0 MB     │
└────────────┴───────┴────────────┴────────────┴────────────┴───────────┴──────────┴──────────┴──────────────┴──────────┘

environment variable ORACLE_SID
┌─────────────────┬───────┬────────────┬────────────┬────────────┬───────────┬──────────┬──────────┬─────────────┬──────────┐
│ group_name      │ procs │ mem_rss    │ mem_anon   │ mem_uss    │ swap_anon │ swap_rss │ swap_uss │ shm_mem     │ shm_swap │
├─────────────────┼───────┼────────────┼────────────┼────────────┼───────────┼──────────┼──────────┼─────────────┼──────────┤
│ Some("+ASM1")   │ 102   │ 5209.41 MB │ 4636.65 MB │ 5179.87 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 3213.41 MB  │ 0 MB     │
│ None            │ 74    │ 3790.02 MB │ 3536.86 MB │ 3765.41 MB │ 0.01 MB   │ 0.01 MB  │ 0.01 MB  │ 0 MB        │ 0 MB     │
│ Some("DBB1")    │ 109   │ 2357.09 MB │ 1489.36 MB │ 2216.93 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 21479.03 MB │ 0 MB     │
│ Some("DBD1")    │ 90    │ 1614.49 MB │ 1101.75 MB │ 1470.30 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 4299.16 MB  │ 0 MB     │
│ Some("DBC1")    │ 92    │ 1483.17 MB │ 968.65 MB  │ 1338.92 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 10741.61 MB │ 0 MB     │
│ Some("DBA1")    │ 77    │ 1439.17 MB │ 944.71 MB  │ 1298.64 MB │ 0 MB      │ 0 MB     │ 0 MB     │ 68723.67 MB │ 0 MB     │
└─────────────────┴───────┴────────────┴────────────┴────────────┴───────────┴──────────┴──────────┴─────────────┴──────────┘
```

If the provided groups are not sufficient, you can use `--split-custom`.
It can be repeated multiple times to compute statistics for multiple groups, but on the same dataset.

```
# memstats groups -c "comm(sshd),and(uid(0),not(comm(bash)))" -c "uid(1001)"
...
Custom splitter
┌─────────────────────────────┬───────┬────────────┬────────────┬────────────┬───────────┬──────────┬──────────┬─────────┬──────────┐
│ group_name                  │ procs │ mem_rss    │ mem_anon   │ mem_uss    │ swap_anon │ swap_rss │ swap_uss │ shm_mem │ shm_swap │
├─────────────────────────────┼───────┼────────────┼────────────┼────────────┼───────────┼──────────┼──────────┼─────────┼──────────┤
│ Other                       │ 67    │ 2062.23 MB │ 1764.56 MB │ 2027.51 MB │ 0.12 MB   │ 0.12 MB  │ 0.12 MB  │ 0.01 MB │ 0 MB     │
│ and(uid(0),not(comm(bash))) │ 30    │ 134.36 MB  │ 53.49 MB   │ 97.91 MB   │ 0.02 MB   │ 0.07 MB  │ 0.07 MB  │ 0 MB    │ 0 MB     │
│ comm(sshd)                  │ 3     │ 14.78 MB   │ 2.28 MB    │ 4.96 MB    │ 0 MB      │ 0 MB     │ 0 MB     │ 0 MB    │ 0 MB     │
└─────────────────────────────┴───────┴────────────┴────────────┴────────────┴───────────┴──────────┴──────────┴─────────┴──────────┘

Custom splitter
┌────────────┬───────┬────────────┬────────────┬────────────┬───────────┬──────────┬──────────┬─────────┬──────────┐
│ group_name │ procs │ mem_rss    │ mem_anon   │ mem_uss    │ swap_anon │ swap_rss │ swap_uss │ shm_mem │ shm_swap │
├────────────┼───────┼────────────┼────────────┼────────────┼───────────┼──────────┼──────────┼─────────┼──────────┤
│ Other      │ 100   │ 2166.83 MB │ 1818.70 MB │ 2166.83 MB │ 0.15 MB   │ 0.19 MB  │ 0.19 MB  │ 0.01 MB │ 0 MB     │
│ uid(1001)  │ 0     │ 0 MB       │ 0 MB       │ 0 MB       │ 0 MB      │ 0 MB     │ 0 MB     │ 0 MB    │ 0 MB     │
└────────────┴───────┴────────────┴────────────┴────────────┴───────────┴──────────┴──────────┴─────────┴──────────┘
```

### How it works
1. list all processes
1. exlude kernel processes, exclude processes not matching filter
1. For each process, compute the set of pages referenced (via `/proc/<pid>/smaps` and `/proc/<pid>/pagemap`)
1. For each process group, compute the union of sets
1. For each group, compute the difference between this groups' set and others', this gives the group USS (memory only referenced by processes in this group). RSS is memory referenced by this group that may also be referenced by processes in other groups

### Building
Grab a precompiled portable build in the [releases](https://github.com/tatref/linux-mem/releases)

Multiple hash functions can be used. Seems that `fxhash` is the fastest

features :
* `--features fxhash` (default)
* `--features ahash`
* `--features fnv`
* `--features metrohash`
* `--features std`


Require a nighly compiler

Require main branch of procfs until v0.16 is released

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
