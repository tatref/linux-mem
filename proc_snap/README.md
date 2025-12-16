# snap.py

Take a "snapshot" of /proc for later inspection

This is not a real snapshot as it is not consistent


## Usage

Test

```
# sudo ./snap.py test
```

Run

``` 
# ./snap.py run /tmp/snap123
22:40:27 line 416 INFO: Dump archive = /tmp/snap123.tar.gz
22:40:27 line 446 INFO: Collecting...

$ ls -ltrh /tmp/snap123.tar.gz 
-rw-r--r-- 1 root root 3.8M Jun 29 22:40 /tmp/snap123.tar.gz
```


## Requirements

* `tar` 1.29 (provides the `SEEK_HOLE` and `SEEK_DATA` syscalls)
* Python 3.6 (Python 3.3 without types annotations)
* Kernel (see `man lseek`):        
  * Btrfs (since Linux 3.1)
  * OCFS (since Linux 3.2)
  * XFS (since Linux 3.5)
  * ext4 (since Linux 3.8)
  * tmpfs(5) (since Linux 3.8)
  * NFS (since Linux 3.18)
  * FUSE (since Linux 4.5)
  * GFS2 (since Linux 4.15)

This tool uses sparse files inside the tar.gz. At the moment, 7-Zip cannot open such a file (see https://sourceforge.net/p/sevenzip/discussion/45797/thread/5b5abf1956/)


### Redhat 6

Not supported

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
# yum install tar
$ tar --version
tar (GNU tar) 1.34
```


### Windows (WIP!)

Opening the archive requires sparse files support

https://learn.microsoft.com/en-us/windows/win32/api/winioctl/ni-winioctl-fsctl_set_zero_data


