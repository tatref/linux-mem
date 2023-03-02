# Linux memory tools

A toolbox to inspect Linux memory

# Small tools
## [shmat](src/bin/shmat.rs)

Attach shared memory segments to current process

## [connections](oracle-tools/src/bin/connections.rs)

Establish lots of connections to Oracle database

## [find_instances](oracle-tools/src/bin/find_instances.rs)

Find Oracle database instances, connect to DB and run some request. Env variables (SID, lib...) and user is found automatically.

## [procinfo](src/bin/procinfo.rs)

Memory map details for single process

# Bigger tools
## [snap.py](proc_snap/snap.py)

/proc snapshot tool

## [groupstats](src/bin/groupstats.rs)

Memory usage for groups of processes. RSS and USS are computed from physical pages allocation, this is not a simple sum of each process.

Groups can be created by user, by environment variable, or by user provided PIDs list

Memory usage for my processes (id -u) and root's, grouped by env variable `SHELL`

```
$ sudo ./groupstats --filter "or(uid(0),uid($(id -u)))" --split-env SHELL
[2023-03-01T23:54:25Z INFO  groupstats] Memory limit: 2063 MiB
[2023-03-01T23:54:25Z INFO  groupstats] Using 8 threads
[2023-03-01T23:54:25Z INFO  groupstats] 
[2023-03-01T23:54:25Z INFO  groupstats] Filter excluded 48 processes, 202 processes remaining
[2023-03-01T23:54:25Z INFO  groupstats] 146 kernel processes
[2023-03-01T23:54:25Z INFO  groupstats] Scanning 56 processes
[2023-03-01T23:54:25Z INFO  groupstats] 
[2023-03-01T23:54:25Z INFO  groupstats] Scanned 55 processes in 72.531698ms
[2023-03-01T23:54:25Z INFO  groupstats] 1 vanished processes
[2023-03-01T23:54:25Z INFO  groupstats::splitters] Split by environment variable SHELL: 18.241642ms
[2023-03-01T23:54:25Z INFO  groupstats::splitters] Process groups by environment variable SHELL
[2023-03-01T23:54:25Z INFO  groupstats::splitters] group_name                     #procs     RSS MiB     USS MiB
[2023-03-01T23:54:25Z INFO  groupstats::splitters] =============================================================
[2023-03-01T23:54:25Z INFO  groupstats::splitters] Some("/bin/sh")                     2          26          14
[2023-03-01T23:54:25Z INFO  groupstats::splitters] None                               30         108          89
[2023-03-01T23:54:25Z INFO  groupstats::splitters] Some("/bin/bash")                  23        2063        2048
[2023-03-01T23:54:25Z INFO  groupstats::splitters] Display split by environment variable SHELL: 16.264999ms
[2023-03-01T23:54:25Z INFO  groupstats::splitters] 
[2023-03-01T23:54:25Z INFO  groupstats] vmhwm = 70468
[2023-03-01T23:54:25Z INFO  groupstats] rssanon = 70468
[2023-03-01T23:54:25Z INFO  groupstats] vmrss = 74188
[2023-03-01T23:54:25Z INFO  groupstats] global_elapsed = 107.808383ms
```

### How it works
1. list all processes
1. exlude kernel processes, and apply filter
1. For each process, compute the set of pages referenced
1. For each process group, compute the union of sets
1. For each group, compute the difference between this groups' set and other, this gives USS (memory only referenced by processes in this group). RSS is memory referenced by this group that may also be referenced by processes in other groups

![Memory groups Venn diagram RSS USS](./assets/Process_groups_RSS_USS.png)

### Building

Require a fork of procfs until PR 254 is merged (https://github.com/eminence/procfs/pull/254)
```
$ ./builder.sh
$ ./build.sh cargo b --release --bin groupstats
$ ll target.el7/release/groupstats
-rwxr-xr-x 2 tatref tatref 9564720 Feb 21 23:02 target.el7/release/groupstats
```

features :
* `--features fxhash` (default)
* `--features ahash`
* `--features fnv`
* `--features metrohash`
* `--features std`

To build all releases:
```
for h in ahash std fnv metrohash fxhash
do
  for arch in x86-64 x86-64-v2 x86-64-v3
  do
    echo $h $arch
    RUSTFLAGS="-C target-cpu=$arch" ./build.sh cargo b --release --bin groupstats --no-default-features --features $h
  done
done
```

## [processes2png](src/bin/processes2png.rs)

Visual map of processes memory

For details, see [my blog post](https://tatref.github.io/blog/2023-visual-linux-memory-compact/)


![gif](https://tatref.github.io/blog/2023-visual-linux-memory-compact/out.gif)

