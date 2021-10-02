#!/usr/bin/env python3


import glob
import shutil
import os
import datetime
import sys
import re
import code


RE_STATUS_FILE = re.compile(r'^([a-zA-Z_]+):\t+(.+)$')

def strings(s):
    return s.split('\x00')


def read(path):
    return open(path).read().strip()


def parse_smaps_file(smaps_file):
    pass


def parse_status_file(status_file):
    status_data = read(status_file)

    status = {}
    for line in status_data.splitlines():
        m = RE_STATUS_FILE.match(line)
        if m:
            key = m.group(1)
            value = m.group(2)
            status[key] = value

    return status


def load_proc_entry(proc_entry):
    cmdline = strings(read(proc_entry + '/cmdline'))
    status = parse_status_file(proc_entry + '/status')
    smaps = parse_smaps_file(proc_entry + '/smaps')

    return {
            'cmdline': cmdline,
            'status': status,
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
    import pprint
    pprint.pprint(snap)


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


