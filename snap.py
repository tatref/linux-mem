#!/usr/bin/env python

import glob
import shutil
import os
import datetime
import subprocess
import shlex
import socket
import sys
#import struct


PAGE_SIZE = 4096

if os.getuid() != 0:
    print('ERROR: run as root to collect all processes and physical memory')
    sys.exit(1)

if sys.version_info[0] < 3:
    print('ERROR: Requires Python 3')
    sys.exit(1)



NOW = datetime.datetime.now().replace(microsecond=0).strftime('%Y-%m-%dT%H_%M_%S')
HOSTNAME = socket.gethostname()

OUT_DIR = sys.argv[1]
#OUT_DIR = 'memory-snapshot-' + HOSTNAME + '-' + NOW
os.makedirs(OUT_DIR)
os.makedirs(OUT_DIR + '/proc')
os.makedirs(OUT_DIR + '/proc/sysvipc')


def parse_proc_pid_maps(path):
    result = []
    f = open(path)
    maps = f.readlines()
    for line in maps:
        split = line.split(' ')
        address = split[0]
        cmd = split[-1]
        start = int(address.split('-')[0], 16)
        end = int(address.split('-')[1], 16)
        result.append((start, end, cmd))
    f.close()
    return result


def dump_pid_pagemap(pid, dest):
    ENTRY_SIZE = 8

    maps = parse_proc_pid_maps('/proc/' + pid + '/maps')
    fi = open('/proc/' + pid + '/pagemap', 'rb')
    fo = open(dest + '/pagemap', 'wb')

    for entry in maps:
        start, end, cmd = entry

        if cmd == '[vsyscall]':
            pass

        vpn_start = start // PAGE_SIZE
        vpn_end = end // PAGE_SIZE
        pages = vpn_end - vpn_start
        offset = vpn_start * ENTRY_SIZE

        fi.seek(offset)
        fo.seek(offset)

        data = fi.read(pages * ENTRY_SIZE)
        fo.write(data)

        #for vp in range(vpn_start, vpn_end):
        #    offset = vp * ENTRY_SIZE
        #    fi.seek(offset)
        #    fo.seek(offset)
        #    data = fi.read(ENTRY_SIZE)
        #    fo.write(data)

    fo.close()
    fi.close()

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

    fi = open('/proc/kpagecount', 'rb')
    fo = open(OUT_DIR + '/proc/kpagecount', 'wb')
    for (start, end, name) in iomem:
        if name != 'System RAM':
            continue

        pfn_start = start // PAGE_SIZE
        pfn_end = end // PAGE_SIZE
        pages = pfn_end - pfn_start
        offset = pfn_start * ENTRY_SIZE

        fi.seek(offset)
        fo.seek(offset)

        data = fi.read(pages * ENTRY_SIZE)
        fo.write(data)


    fi.close()
    fo.close()
        

def dump_kpageflags(iomem):
    ENTRY_SIZE = 8

    fi = open('/proc/kpageflags', 'rb')
    fo = open(OUT_DIR + '/proc/kpageflags', 'wb')
    for (start, end, name) in iomem:
        if name != 'System RAM':
            continue

        pfn_start = start // PAGE_SIZE
        pfn_end = end // PAGE_SIZE
        pages = pfn_end - pfn_start
        offset = pfn_start * ENTRY_SIZE

        fi.seek(offset)
        fo.seek(offset)

        data = fi.read(pages * ENTRY_SIZE)
        fo.write(data)

    fi.close()
    fo.close()
        


############################################
##                 MAIN                   ##
############################################



print('INFO: Collecting...')
for cmd in ['getconf -a']:
    try:
        out = subprocess.check_output(shlex.split(cmd))
        f = open(OUT_DIR + '/' + cmd.replace(' ', '_'), 'w')
        f.write(str(out))
        f.close()
    except Exception as e:
        print('WARNING: command + "' + cmd + '" failed: ' + str(e))


print('INFO: Dumping kernel info...')
iomem = parse_proc_iomem()
dump_kpagecount(iomem)
dump_kpageflags(iomem)


for f in ['iomem', 'cmdline', 'meminfo', 'vmstat', 'pagetypeinfo', 'slabinfo', 'sysvipc/shm']:
    try:
        shutil.copyfile('/proc/' + f, OUT_DIR + '/proc/' + f)
    except:
        print('WARNING: Skipping: /proc/' + f)


print('INFO: Dumping processes...')
for proc in glob.glob('/proc/[0-9]*'):
    pid = proc[6:]
    
    dest = OUT_DIR + '/proc/' + pid
    os.makedirs(dest)

    try:
        try:
            dump_pid_pagemap(pid, dest)
        except Exception as e:
            print("WARNING: failed to dump pagemap for {}".format(pid))
            #continue

        for f in ['cmdline', 'maps', 'smaps', 'status', 'stat', 'environ']:
            shutil.copyfile(proc + '/' + f, dest + '/' + f)
        for f in ['exe', 'root']:
            try:
                # try to read exe (kernel procs)
                os.readlink(proc + '/' + f)
            except:
                continue
            shutil.copyfile(proc + '/' + f, dest + '/' + f, follow_symlinks=False)
    except Exception as e:
        print('WARNING: Skipping PID ' + pid + ': ' + str(e))
        shutil.rmtree(dest)

print('INFO: Compressing archive...')
ret = subprocess.call(shlex.split('tar czf ' + OUT_DIR + '.tar.gz --sparse ' + OUT_DIR))
if ret != 0:
    print('ERROR: tar failed')
    sys.exit(1)

shutil.rmtree(OUT_DIR)
print('INFO: Done ' + OUT_DIR + '.tar.gz')
