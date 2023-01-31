#!/usr/bin/env python
# TODO:
#  * type hints https://mypy.readthedocs.io/en/stable/cheat_sheet_py3.html#variables
# mypy --allow-redefinition snap.py
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

import sys
import os

from pathlib import Path
import argparse
import datetime
import glob
import json
import logging
import shlex
import shutil
import socket
import subprocess
import time
from typing import List, Set, Dict, Tuple, Optional, Union, Any

profile = False
if profile:
    from pstats import SortKey
    import cProfile, pstats, io



PAGE_SIZE = os.sysconf('SC_PAGE_SIZE')



def check_kernel() ->bool:
    #kernel = os.uname().release
    #kernel_version = tuple(int(x) for x in kernel.split('-')[0].split('.'))
    #print(kernel_version)
    #if kernel_version < (1, 2, 3):
    #    print('ERROR: kernel is too old')
    #    sys.exit(1)
    return True


def check_tar_version() -> bool:
    output = subprocess.check_output(shlex.split("tar --version")).splitlines()[0]
    version = output.split(b' ')[-1]
    version = tuple(int(x) for x in version.split(b'.'))
    if version < (1, 29):
        return False
    return True


def parse_getconf() -> Dict[str, Optional[str]]:
    try:
        result: Dict[str, Optional[str]] = {}
        out = subprocess.check_output(shlex.split('getconf -a'))
        for line in out.splitlines():
            l =  str(line, 'utf-8').split()
            if len(l) == 1:
                result[l[0]] = None
            else:
                result[l[0]] = ' '.join(l[1:])
        return result
    except Exception as e:
        logging.warning("Can't run 'getconf': {}", e)
        return {}


def parse_proc_pid_maps(path) -> List[Tuple[int, int, str]]:
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


def dump_pid_pagemap(pid, dest):
    ENTRY_SIZE = 8

    data_size = 0

    maps = parse_proc_pid_maps('/proc/' + pid + '/maps')
    fi = open('/proc/' + pid + '/pagemap', 'rb')
    if mode == 'run':
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

        if mode == 'run':
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if mode == 'run':
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


def dump_kpagecount(iomem):
    ENTRY_SIZE = 8

    data_size = 0

    fi = open('/proc/kpagecount', 'rb')
    if mode == 'run':
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

        if mode == 'run':
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if mode == 'run':
        fo.close()

    return data_size
        

def dump_kpageflags(iomem):
    ENTRY_SIZE = 8

    data_size = 0

    fi = open('/proc/kpageflags', 'rb')
    if mode == 'run':
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

        if mode == 'run':
            fo.seek(offset)
            fo.write(data)

    fi.close()
    if mode == 'run':
        fo.close()

    return data_size
        

def handle_proc_pid(proc_pid):
    data_size = 0
    pid = proc_pid[6:]
    
    if mode == 'run':
        dest = dump_dir / 'proc/' / pid
        os.makedirs(dest)
    else:
        dest = None


    try:
        try:
            data_size += dump_pid_pagemap(pid, dest)
        except Exception as e:
            logging.warning("Failed to dump pagemap for {}".format(pid))
            logging.debug(e)

        # handle files
        for (proc_file, mandatory) in [
            ('cmdline', True),
            ('maps', True),
            ('smaps', True),
            #('smaps_rollup', False),
            ('status', True),
            ('stat', True),
            ('statm', True),
            ('environ', True)]:
            if mode == 'run':
                shutil.copyfile(proc_pid + '/' + proc_file, dest / proc_file)
                file_size = os.stat(dest / proc_file).st_size
            else:
                with open(proc_pid + '/' + proc_file, 'rb') as f:
                    file_size = len(f.read())

            disk_usage = (file_size // block_size) + 1 * block_size
            data_size += disk_usage

        # handle links
        for proc_file in ['exe', 'root']:
            try:
                # try to read exe (kernel procs)
                os.readlink(proc_pid + '/' + proc_file)
            except:
                continue
            if mode == 'run':
                shutil.copyfile(proc_pid + '/' + proc_file, dest / proc_file, follow_symlinks=False)
    except Exception as e:
        print('WARNING: Skipping PID ' + pid + ': ' + str(e))
        if mode == 'run':
            shutil.rmtree(dest)

    return data_size


def test_seek_hole(dump_dir):
    test_file_name = dump_dir.parent / 'test_seek'
    try:
        os.stat(test_file_name)
        logging.critical('Test file already exists: "{}"'.format(test_file_name))
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




parser = argparse.ArgumentParser(description="Linux memory snapshot")
parser.add_argument('--verbose', '-v', action='store_true', help="Verbose")
subparsers = parser.add_subparsers(help='Mode of operation', dest='mode')
subparsers.required = True
parser_dump = subparsers.add_parser('run', help="Generate a dump")
parser_dump = parser_dump.add_argument('dump_dir', help="Path to create the archive. `.tar.gz` is appended.")
parser_test = subparsers.add_parser('test', help="Dry run. Provide statistics")
args = parser.parse_args()

mode = args.mode
if mode == 'run':
    dump_dir: Path = Path(args.dump_dir)
verbose = args.verbose


if verbose:
    loglevel = logging.DEBUG
else:
    loglevel = logging.INFO
if sys.version_info < (3, 9):
    logging.basicConfig(level=loglevel, format='%(asctime)s line %(lineno)d %(levelname)s: %(message)s', datefmt='%I:%M:%S')
else:
    logging.basicConfig(encoding='utf-8', level=loglevel, format='%(asctime)s line %(lineno)d %(levelname)s: %(message)s', datefmt='%I:%M:%S')

logging.debug(args)

if mode == 'run':
    logging.info('Tmp path = %s', dump_dir.absolute())
    logging.info('Dump archive = %s.tar.gz', dump_dir.absolute().with_suffix('.tar.gz'))

if os.geteuid() != 0:
    logging.critical('Run as root / sudo')
    sys.exit(1)

if mode == 'run' and not test_seek_hole(dump_dir):
    logging.critical('Mount point does not support SEEK_HOLE')
    sys.exit(1)

if mode == 'test':
    logging.info('Test mode: no file will be generated')

if mode == 'run':
    os.makedirs(dump_dir)
    os.makedirs(dump_dir / 'proc')



# FS block size
block_size = int(subprocess.check_output(shlex.split("stat -fc %s .")))
# Running size of copied data for test mode
data_size = 0


global_chrono = time.perf_counter()
logging.info('Collecting...')

if profile:
    pr = cProfile.Profile()
    pr.enable()

metadata: Dict[str, Any] = {}
metadata['hostname'] = socket.gethostname()
metadata['datetime'] = datetime.datetime.now().isoformat()
metadata['getconf'] = parse_getconf()


logging.info('Dumping kernel info...')
kernel_chrono = time.perf_counter()

iomem = parse_proc_iomem()
data_size += dump_kpagecount(iomem)
data_size += dump_kpageflags(iomem)


for proc_file in ['iomem', 'cmdline', 'meminfo', 'vmstat', 'buddyinfo', 'pagetypeinfo', 'slabinfo', 'sysvipc/shm', 'swaps', 'zoneinfo']:
    proc_file = Path(proc_file)
    try:
        if mode == 'run':
            os.makedirs((dump_dir / 'proc' / proc_file).parent, exist_ok=True)
            shutil.copyfile('/proc' / proc_file, dump_dir / 'proc' / proc_file)
    except Exception as e:
        logging.warning('Skipping: /proc/' + proc_file)
        logging.debug(e)
kernel_duration = time.perf_counter() - kernel_chrono
kernel_duration = datetime.timedelta(seconds=kernel_duration)


logging.info('Dumping processes...')
processes_chrono = time.perf_counter()

proc_pids = glob.glob('/proc/[0-9]*')
n_procs = len(proc_pids)
for proc_pid in proc_pids:
    data_size += handle_proc_pid(proc_pid)

processes_duration = time.perf_counter() - processes_chrono
processes_duration = datetime.timedelta(seconds=processes_duration)


if profile:
    pr.disable()
    s = io.StringIO()
    sortby = SortKey.CUMULATIVE
    ps = pstats.Stats(pr, stream=s).sort_stats(sortby)
    ps.print_stats()
    print(s.getvalue())


def compress_tar_gz(dump_dir: Path):
    logging.info('Compressing archive using tar...')
    arc = dump_dir.with_suffix('.tar.gz').as_posix()
    cmd = 'tar czf ' + arc + ' --sparse -C ' + dump_dir.parent.as_posix() + ' ' + dump_dir.name
    logging.debug(cmd)
    ret = subprocess.call(shlex.split(cmd))
    if ret != 0:
        logging.critical('tar failed')
        sys.exit(1)

    shutil.rmtree(dump_dir)
    logging.info('Done ' + arc + '.tar.gz')


metadata['timings'] = {}
metadata['timings']['kernel_duration'] = str(kernel_duration)
metadata['timings']['processes_duration'] = str(processes_duration)

if mode == 'run':
    with open(dump_dir / "metadata.json", 'x') as f:
        json.dump(metadata, f, indent=4, sort_keys=True, default=str)


if mode == 'run':
    compress_chrono = time.perf_counter()
    compress_tar_gz(dump_dir)
    compress_duration: Any = time.perf_counter() - compress_chrono
else:
    compress_duration = None

global_duration = datetime.timedelta(seconds=time.perf_counter() - global_chrono)
if mode == 'run':
    compress_duration = datetime.timedelta(seconds=compress_duration)

logging.info('Total duration: {}'.format(global_duration))
logging.info('Kernel duration: {}'.format(kernel_duration))
logging.info('Processes duration: {}'.format(processes_duration))
logging.info('Processes {}'.format(n_procs))
logging.info('Compression duration: {}'.format(compress_duration))
logging.info('Statistics: read data: {:.2f} MiB'.format(data_size / 1024 / 1024))
logging.info('Statistics: estimated disk usage: {:.2f} MiB'.format(data_size * 2 / 1024 / 1024))
