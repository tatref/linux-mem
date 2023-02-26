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

```
Process groups by UID
group_name                     #procs     RSS MiB     USS MiB
=============================================================
root                               31          80          60
gdm                                39         122         109
avahi                               2           5           1
dbus                                2           6           2
rtkit                               1           3           0
chrony                              1           2           0
libstoragemgmt                      1           2           0
colord                              1           7           1
polkitd                             1          12           5
tatref                             23        1118        1102
```

### Building

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

