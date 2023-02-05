# Linux memory tools

A toolbox to inspect Linux memory

# Small tools
## [shmat](src/bin/shmat)

Attach shared memory segments to current process

## [connections](oracle-tools/src/bin/connections.rs)

Establish lots of connections to Oracle database

## [find_instances](oracle-tools/src/bin/find_instances.rs)

Find Oracle database instances, connect to DB and run some request. Env variables (SID, lib...) and user is found automatically.

# Bigger tools
## [snap.py](proc_snap/snap.py)

/proc snapshot tool

## [procstats2](src/bin/procstats2.rs)

Memory usage for groups of processes. RSS and USS are computed from physical pages allocation, this is not a simple sum of each process.

Groups can be created by user, by environment variable, or by user provided PIDs list

```
Processes by user:
user root                      RSS    204 MiB USS    170 MiB
user gdm                       RSS    361 MiB USS    333 MiB
user avahi                     RSS      6 MiB USS      1 MiB
user dbus                      RSS      7 MiB USS      3 MiB
user rtkit                     RSS      3 MiB USS      0 MiB
user chrony                    RSS      4 MiB USS      1 MiB
user libstoragemgmt            RSS      1 MiB USS      0 MiB
user colord                    RSS     11 MiB USS      2 MiB
user polkitd                   RSS     26 MiB USS     16 MiB
user tatref                    RSS   1966 MiB USS   1946 MiB
```

## [processes2png](src/bin/processes2png.rs)

Visual map of processes memory

For examples, see [my blog](https://tatref.github.io/blog/2023-visual-linux-memory-compact/)

