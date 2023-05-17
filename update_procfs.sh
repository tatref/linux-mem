#!/bin/bash

set -euo pipefail

pushd procfs
git switch master
git reset --hard
git pull
git branch -D tatref
git switch -c tatref
git rebase bitflags-v2
git push -f tatref tatref
popd

