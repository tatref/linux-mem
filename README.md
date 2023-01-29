# Linux memory tools

A toolbox to inspect / play with Linux memory

# Requirements

* `tar` => 1.29 is required (it provides the `SEEK_HOLE` and `SEEK_DATA` syscalls)
* Python 3 is required
* Kernel (see `man lseek`):        
  * Btrfs (since Linux 3.1)
  * OCFS (since Linux 3.2)
  * XFS (since Linux 3.5)
  * ext4 (since Linux 3.8)
  * tmpfs(5) (since Linux 3.8)
  * NFS (since Linux 3.18)
  * FUSE (since Linux 4.5)
  * GFS2 (since Linux 4.15)


## OEL 7

Compiling latest `tar`

```
yum install gcc
curl -O https://ftp.gnu.org/gnu/tar/tar-1.34.tar.gz
tar xf tar-1.34.tar.gz
cd tar-1.34
FORCE_UNSAFE_CONFIGURE=1 ./configure
make
./src/tar --version
```

Python 3.6

```
yum install python36
```

Add the compiled directory to the `$PATH`

```
export PATH=./tar-1.34/src:$PATH
python3 ./snap.py dump
```

## OEL 8

TODO

## OEL 9

```
yum install tar
```