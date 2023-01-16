#!/usr/bin/env python
#
# Tested kernels:
# - 5.15 (UEK7)
#
#
# Not yet tested:
# - 5.4 (UEK6)
# - 4.14 (UEK5)
# - 4.1 (UEK4)
# - 3.8 (UEK3)
# - 2.6.39 (UEKR2)
#
# See https://blogs.oracle.com/scoter/post/oracle-linux-and-unbreakable-enterprise-kernel-uek-releases
#


from pathlib import Path
import argparse
import datetime
import glob
import logging
import os
import shlex
import shutil
import subprocess
import sys
import time


PAGE_SIZE = 4096




def check_tar_version():
    output = subprocess.check_output(shlex.split("tar --version")).splitlines()[0]
    version = output.split(b' ')[-1]
    version = tuple(int(x) for x in version.split(b'.'))
    if version < (1, 29):
        return False
    return True


def parse_proc_pid_maps(path):
    result = []
    f = open(path)
    maps = f.readlines()
    for line in maps:
        split = line.split(' ')
        address = split[0]
        path = split[-1]
        start = int(address.split('-')[0], 16)
        end = int(address.split('-')[1], 16)
        result.append((start, end, path))
    f.close()
    return result


def dump_pid_pagemap(pid, dest, dry_run=False):
    ENTRY_SIZE = 8

    data_size = 0

    maps = parse_proc_pid_maps('/proc/' + pid + '/maps')
    fi = open('/proc/' + pid + '/pagemap', 'rb')
    if not dry_run:
        fo = open(dest / 'pagemap', 'wb')

    for entry in maps:
        start, end, path = entry

        if path == '[vsyscall]':
            pass

        vpn_start = start // PAGE_SIZE
        vpn_end = end // PAGE_SIZE
        pages = vpn_end - vpn_start
        offset = vpn_start * ENTRY_SIZE

        fi.seek(offset)
        data = fi.read(pages * ENTRY_SIZE)
        data_size += pages * ENTRY_SIZE

        if not dry_run:
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if not dry_run:
        fo.close()

    return data_size


def parse_proc_iomem():
    result = []
    f = open('/proc/iomem')
    iomem = f.readlines()
    for line in iomem:
        address, name = line.split(' : ')
        start = int(address.split('-')[0], 16)
        end = int(address.split('-')[1], 16)
        result.append((start, end, name.strip()))
    f.close()
    return result


def dump_kpagecount(iomem, dry_run=False):
    ENTRY_SIZE = 8

    data_size = 0

    fi = open('/proc/kpagecount', 'rb')
    if not dry_run:
        fo = open(dump_dir / 'proc/kpagecount', 'wb')
    for (start, end, name) in iomem:
        if name != 'System RAM':
            continue

        pfn_start = start // PAGE_SIZE
        pfn_end = end // PAGE_SIZE
        pages = pfn_end - pfn_start
        offset = pfn_start * ENTRY_SIZE

        fi.seek(offset)
        data = fi.read(pages * ENTRY_SIZE)
        data_size += pages * ENTRY_SIZE

        if not dry_run:
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if not dry_run:
        fo.close()

    return data_size
        

def dump_kpageflags(iomem, dry_run=False):
    ENTRY_SIZE = 8

    data_size = 0

    fi = open('/proc/kpageflags', 'rb')
    if not dry_run:
        fo = open(dump_dir / 'proc/kpageflags', 'wb')
    for (start, end, name) in iomem:
        if name != 'System RAM':
            continue

        pfn_start = start // PAGE_SIZE
        pfn_end = end // PAGE_SIZE
        pages = pfn_end - pfn_start
        offset = pfn_start * ENTRY_SIZE


        fi.seek(offset)
        data = fi.read(pages * ENTRY_SIZE)
        data_size += pages * ENTRY_SIZE

        if not dry_run:
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if not dry_run:
        fo.close()

    return data_size
        

def handle_proc_pid(proc_pid):
    data_size = 0
    pid = proc_pid[6:]
    
    dest = dump_dir / 'proc/' / pid
    if not dry_run:
        os.makedirs(dest)

    try:
        try:
            data_size += dump_pid_pagemap(pid, dest, dry_run=dry_run)
        except Exception as e:
            print("WARNING: failed to dump pagemap for {}".format(pid))
            if verbose:
                print(e)
            #continue

        # handle files
        for proc_file in ['cmdline', 'maps', 'smaps', 'status', 'stat', 'environ']:
            if dry_run:
                with open(proc_pid + '/' + proc_file, 'rb') as f:
                    file_size = len(f.read())
            else:
                shutil.copyfile(proc_pid + '/' + proc_file, dest / proc_file)
                file_size = os.stat(dest / proc_file).st_size

            disk_usage = (file_size // block_size) + 1 * block_size
            data_size += disk_usage

        # handle links
        for proc_file in ['exe', 'root']:
            try:
                # try to read exe (kernel procs)
                os.readlink(proc_pid + '/' + proc_file)
            except:
                continue
            if not dry_run:
                shutil.copyfile(proc_pid + '/' + proc_file, dest / proc_file, follow_symlinks=False)
    except Exception as e:
        print('WARNING: Skipping PID ' + pid + ': ' + str(e))
        if not dry_run:
            shutil.rmtree(dest)

    return data_size


def test_seek_hole(dump_dir):
    test_file_name = dump_dir.parent / 'test_seek'
    try:
        os.stat(test_file_name)
        print('ERROR: test file already exists: "{}"'.format(test_file_name))
        sys.exit(1)
    except Exception as e:
        # no file
        pass

    seek_offset = 1024 * 1024

    f = open(test_file_name, 'xb')
    f.seek(seek_offset)
    f.write(b'hello')
    f.seek(0)
    data_offset = f.seek(0, os.SEEK_DATA)
    f.close()

    os.remove(test_file_name)

    if data_offset != seek_offset:
        return False
    else:
        return True





############################################
##                 MAIN                   ##
############################################

if sys.version_info[0] < 3:
    print('ERROR: Requires Python 3')
    sys.exit(1)

if os.uname().sysname != 'Linux':
    print('ERROR: Linux only')
    sys.exit(1)

if not check_tar_version():
    print('ERROR: require tar >= 1.29 to compress archive')
    sys.exit(1)


#kernel = os.uname().release
#kernel_version = tuple(int(x) for x in kernel.split('-')[0].split('.'))
#print(kernel_version)
#if kernel_version < (1, 2, 3):
#    print('ERROR: kernel is too old')
#    sys.exit(1)


parser = argparse.ArgumentParser(description="Linux memory snapshot")
parser.add_argument('dump_dir', help="Path to create the archive. `.tar.gz` is appended.")
parser.add_argument('--dry_run', action='store_true', help="Don't create archive, only output statistics.")
parser.add_argument('--verbose', action='store_true', help="Verbose")

args = parser.parse_args()
dry_run = args.dry_run
dump_dir = Path(args.dump_dir)
verbose = args.verbose


if verbose:
    loglevel = logging.DEBUG
else:
    loglevel = logging.INFO
logging.basicConfig(encoding='utf-8', level=loglevel, format='%(asctime)s %(levelname)s: %(message)s', datefmt='%I:%M:%S')


logging.info('Tmp path = %s', dump_dir.absolute())
logging.info('Dump archive = %s.tar.gz', dump_dir.absolute().with_suffix('.tar.gz'))

if os.geteuid() != 0:
    logging.critical('Run as root / sudo')
    sys.exit(1)

if not test_seek_hole(dump_dir):
    logging.critical('Mount point does not support SEEK_HOLE')
    sys.exit(1)

if dry_run:
    logging.info('dry_run')

if not dry_run:
    os.makedirs(dump_dir)
    os.makedirs(dump_dir / 'proc')
    os.makedirs(dump_dir / 'proc/sysvipc')



block_size = int(subprocess.check_output(shlex.split("stat -fc %s .")))
data_size = 0


start_time = time.perf_counter()
logging.info('Collecting...')
for cmd in ['getconf -a']:
    try:
        out = subprocess.check_output(shlex.split(cmd))
        if not dry_run:
            proc_file = open(dump_dir / cmd.replace(' ', '_'), 'w')
            proc_file.write(str(out))
            proc_file.close()
    except Exception as e:
        logging.warning('command + "' + cmd + '" failed: ' + str(e))


logging.info('Dumping kernel info...')
iomem = parse_proc_iomem()
data_size += dump_kpagecount(iomem, dry_run=dry_run)
data_size += dump_kpageflags(iomem, dry_run=dry_run)


for proc_file in ['iomem', 'cmdline', 'meminfo', 'vmstat', 'buddyinfo', 'pagetypeinfo', 'slabinfo', 'sysvipc/shm']:
    try:
        if not dry_run:
            shutil.copyfile('/proc/' + proc_file, dump_dir / 'proc' / proc_file)
    except:
        logging.warning('Skipping: /proc/' + proc_file)


logging.info('Dumping processes...')
proc_pids = glob.glob('/proc/[0-9]*')


for proc_pid in proc_pids:
    data_size += handle_proc_pid(proc_pid)


def compress_tar_gz(dump_dir):
    logging.info('Compressing archive using tar...')
    arc = dump_dir.with_suffix('.tar.gz').as_posix()
    ret = subprocess.call(shlex.split('tar czf ' + arc + ' --sparse ' + dump_dir.as_posix()))
    if ret != 0:
        logging.critical('tar failed')
        sys.exit(1)

    shutil.rmtree(dump_dir)
    logging.info('Done ' + arc + '.tar.gz')



if not dry_run:
    compress_tar_gz(dump_dir)

elapsed_time = datetime.timedelta(seconds=time.perf_counter() - start_time)

logging.info('Elapsed time: {}'.format(elapsed_time))
logging.info('Statistics: data_size: {:.2f} MiB'.format(data_size / 1024 / 1024))
logging.info('Statistics: estimated disk usage: {:.2f} MiB'.format(data_size * 2 / 1024 / 1024))
