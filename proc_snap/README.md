# snap.py

Take a snapshot of /proc for later inspection

Snapshot is inconsistent


## Requirements

* `tar` => 1.29 is required (provides the `SEEK_HOLE` and `SEEK_DATA` syscalls)
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


### Redhat 6

Not supported, Python does not provide os.SEEK_DATA

Python 3.11 requires std=c11, that gcc 4.4 can not compile

```
yum install gcc
yum install libffi-devel
curl -LO https://www.python.org/ftp/python/3.10.11/Python-3.10.11.tar.xz
tar xf Python-3.10.11.tar.xz
cd Python-3.10.11/
./configure --enable-optimizations
make -j8
```

```
curl -O https://ftp.gnu.org/gnu/tar/tar-1.34.tar.gz
tar xf tar-1.34.tar.gz
cd tar-1.34
FORCE_UNSAFE_CONFIGURE=1 ./configure
make
./src/tar --version
```

```
export PATH=/root/tar-1.34/src:/root/Python-3.10.11:$PATH
python ./snap.py run /path/to/dump
```

### Redhat 7

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
python3 ./snap.py run /path/to/dump
```

### Redhat 8

TODO

### Redhat 9

```
yum install tar
```
