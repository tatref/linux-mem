#!/usr/bin/env python

import glob
import shutil
import os
import datetime
import subprocess
import shlex
import socket


NOW = datetime.datetime.now().replace(microsecond=0).strftime('%Y-%m-%dT%H_%M_%S')
HOSTNAME = socket.gethostname()
OUT_DIR = 'memory-snapshot-' + HOSTNAME + '-' + NOW
os.makedirs(OUT_DIR)


if os.getuid() != 0:
    print('WARNING: run as root to collect all processes')

print('Collecting...')
for cmd in ['getconf -a']:
    try:
        out = subprocess.check_output(shlex.split(cmd))
        f = open(OUT_DIR + '/' + cmd.replace(' ', '_'), 'w')
        f.write(str(out))
        f.close()
    except Exception as e:
        print('WARNING: command + "' + cmd + '" failed: ' + str(e))



os.makedirs(OUT_DIR + '/proc')
os.makedirs(OUT_DIR + '/proc/sysvipc')

for f in ['cmdline', 'meminfo', 'vmstat', 'slabinfo', 'sysvipc/shm']:
    try:
        shutil.copyfile('/proc/' + f, OUT_DIR + '/proc/' + f)
    except:
        print('WARNING: Skipping: /proc/' + f)

for proc in glob.glob('/proc/[0-9]*'):
    pid = proc[6:]
    
    dest = OUT_DIR + '/proc/' + pid
    os.makedirs(dest)

    try:
        for f in ['cmdline', 'smaps', 'status', 'stat', 'environ']:
            shutil.copyfile(proc + '/' + f, dest + '/' + f)
        for f in ['exe', 'root']:
            try:
                # try to read exe (kernel procs)
                os.readlink(f)
            except:
                continue
            shutil.copyfile(proc + '/' + f, dest + '/' + f, follow_symlinks=False)
    except:
        print('WARNING: Skipping PID ' + pid)
        shutil.rmtree(dest)

print('Compressing archive...')
ret = subprocess.call(shlex.split('tar czf ' + OUT_DIR + '.tar.gz ' + OUT_DIR))
if ret != 0:
    print('tar failed')
    sys.exit(1)

shutil.rmtree(OUT_DIR)
print('Done: ' + OUT_DIR + '.tar.gz')
