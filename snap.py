#!/usr/bin/env python

import glob
import shutil
import os
import datetime
import subprocess
import shlex


NOW = datetime.datetime.now().replace(microsecond=0).strftime('%Y-%m-%dT%H_%M_%S')
OUT_DIR = 'memory-snapshot-' + NOW


print('Collecting...')
os.makedirs(OUT_DIR + '/proc')
os.makedirs(OUT_DIR + '/proc/sysvipc')

for f in ['cmdline', 'meminfo', 'vmstat', 'slabinfo', 'sysvipc/shm']:
    try:
        shutil.copyfile('/proc/' + f, OUT_DIR + '/proc/' + f)
    except:
        print('Skipping: /proc/' + f)

for proc in glob.glob('/proc/[0-9]*'):
    try:
        pid = proc[6:]
        
        dest = OUT_DIR + '/proc/' + pid
        os.makedirs(dest)

        for f in ['cmdline', 'smaps', 'status']:
            shutil.copyfile(proc + '/' + f, dest + '/' + f)
    except:
        print('Skipping PID ' + pid)
        shutil.rmtree(dest)

print('Compressing archive...')
ret = subprocess.call(shlex.split('tar czf ' + OUT_DIR + '.tar.gz ' + OUT_DIR))
if ret != 0:
    print('tar failed')
    sys.exit(1)

shutil.rmtree(OUT_DIR)
print('Done: ' + OUT_DIR + '.tar.gz')
