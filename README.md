# Linux memory tools

A toolbox to inspect Linux memory


# Big tools
## [procinfo](src/bin/procinfo.rs)

Memory map details for single process

## [snap.py](proc_snap/README.md)

/proc snapshot tool

## [memstats](src/bin/memstats.rs)

Memory usage for groups of processes. RSS and USS are computed from physical pages allocation, this is not a simple sum of each process.

Groups can be created by user, by environment variable, by user provided PIDs list, or by custom filters

Example invocation:

```
$ sudo ./memstats groups --split-uid --split-env ORACLE_SID
Scanning /proc/kpageflags...
Scanning Oracle instances...
Oracle instances (MiB):
SID                  SGA         PGA  PROCESSES  LARGE_PAGES
============================================================
orcl                24512        279         69         TRUE

Scanning shm...
Shared memory segments (MiB):
         key           id       Size        RSS       4k/2M          SWAP   USED%        SID
============================================================================================
           0            2      22528      22528  5767168/0              0  100.00       orcl
           0            1       1984       1984        0/992            0  100.00       orcl
           0            3         54         54    14015/0              0  100.00       orcl
           0            0         10         10        0/5              0  100.00       orcl
  1966876864            4          0          0       17/0              0  100.00       orcl


Scanning 93 processes
Scanned 92 processes in 79.053151ms
Process groups by UID (MiB)
group_name                     #procs         RSS         USS   SWAP RSS   SWAP USS    SHM MEM   SHM SWAP
=========================================================================================================
oracle                             60       25102       25099          0          0      24576          0
root                               27          95          85          0          0          0          0
polkitd                             1          12           6          0          0          0          0
postfix                             2           7           2          0          0          0          0
dbus                                1           4           0          0          0          0          0
rpc                                 1           3           0          0          0          0          0

Process groups by environment variable ORACLE_SID (MiB)
group_name                     #procs         RSS         USS   SWAP RSS   SWAP USS    SHM MEM   SHM SWAP
=========================================================================================================
Some("orcl")                       60       25102       25099          0          0      24576          0
None                               32         106         103          0          0          0          0
```

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
