#!/usr/bin/env python3


import glob
import shutil
import os
import datetime
import sys
import re
import code
import pprint


class MemoryMapping:
    def __init__(self, address_start, address_end, perm_r, perm_w, perm_x, perm_s, perm_p, offset, dev, inode, pathname):
        self.address_start = address_start
        self.address_end = address_end
        self.perm_read = perm_r
        self.perm_write = perm_w
        self.perm_execute = perm_x
        self.perm_shared = perm_s
        self.perm_private = perm_p
        self.offset = offset
        self.dev = dev
        self.inode = inode
        self.pathname = pathname


RE_STATUS_FILE = re.compile(r'^([a-zA-Z_]+):\t+(.+)$')
RE_SMAPS_MEM_REGION = re.compile(r'([a-f0-9]+)-([a-f0-9]+ (.)(.)(.)(.) ([a-f0-9]+ ([a-f0-9]{2}:[a-f0-9]{2}) (\d+) +(\S+)?')


def strings(s):
    return s.split('\x00')


def read(path):
    return open(path).read().strip()


def parse_smaps_file(smaps_file):
    for line in read(smaps_file).splitlines():
        m = RE_SMAPS_MEM_REGION.match(line)
        if m:
            offset_start = int(m.group(1), 16)
            offset_end = int(m.group(2), 16)

            perm_r = m.group(3) == 'r'
            perm_w = m.group(4) == 'w'
            perm_x = m.group(5) == 'x'
            perm_s =  m.group(6) == 's'
            perm_p =  m.group(6) == 'p'

            offset = int(m.group(7), 16)

            dev = m.group(8)

            inode = int(m.group(9))
            
            if m.group(10) == None:
                pathname = None
            else:
                pathname = m.group(10)


            print(m.group(10))

            memory_mapping = MemoryMapping(offset_start, offset_end, perm_r, perm_w, perm_x, perm_s, perm_p, offset, dev, inode, pathname)




def load_proc_entry(proc_entry):
    cmdline = strings(read(proc_entry + '/cmdline'))
    smaps = parse_smaps_file(proc_entry + '/smaps')

    return {
            'cmdline': cmdline,
            'smaps': smaps,
            }


def unarchive_snap(archive):
    if not os.path.exists(archive.replace('.tar.gz', '')):
        shutil.unpack_archive(archive)


def load_snap(snap_path):
    snap = {}
    snap['cmdline'] = read(snap_path + '/proc/cmdline')
    snap['processes'] = {}

    for proc_entry in glob.glob(snap_path + '/proc/[0-9]*'):
        pid = proc_entry.replace(snap_path + '/proc/', '')
        proc = load_proc_entry(proc_entry)
        snap['processes'][pid] = proc
    #pprint.pprint(snap)


try:
    a = sys.argv[1]
    b = sys.argv[2]
except:
    print('Usage: ' + sys.argv[0] + ' <memory_snap_1> <memory_snap_2>')
    sys.exit(1)


unarchive_snap(a)
snap_a = a.replace('.tar.gz', '')
a = load_snap(snap_a)
#b = load_snap(b)


