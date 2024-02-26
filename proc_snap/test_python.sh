#!/bin/bash
#
# Build Python 3.6 -> 3.12
# Run tests
#


declare -A version_list
version_list=(
	["3.5"]="3.5.10"
	["3.6"]="3.6.15"
	["3.7"]="3.7.17"
	["3.8"]="3.8.18"
	["3.9"]="3.9.18"
	["3.10"]="3.10.13"
	["3.11"]="3.11.8"
	["3.12"]="3.12.2"
)


build_python() {
	version="${version_list[$1]}"


	if [ ! -e "python/Python-$version/python" ]
	then
		if [ ! -e "python/Python-$version.tar.xz" ]
		then
			link="https://www.python.org/ftp/python/$version/Python-$version.tar.xz"
			curl --create-dirs -O --output-dir python/ "$link"
		fi
		pushd python/

		tar xf Python-$version.tar.xz
		pushd Python-$version
		#./configure --enable-optimizations
		./configure
		make -j4

		popd
		popd

	fi

	ls -l "python/Python-$version/python"
}


versions="$(seq -f "3.%g" 6 12)"

for version in $versions
do
	build_python $version
done

failed=0

echo
echo TEST
for version in $versions
do
	version="${version_list[$version]}"
	if output=$(sudo "python/Python-$version/python" ./snap.py test 2>&1)
	then
		printf "%7s... OK\n" "$version"
	else
		echo "$output"
		printf "%7s... FAILED\n" "$version"
		failed=1
	fi
done


echo
echo RUN

for version in $versions
do
	version="${version_list[$version]}"
	outdir=$(sudo mktemp -d temp_proc_snap_XXXXX)
	if output=$(sudo "python/Python-$version/python" ./snap.py run "$outdir" 2>&1)
	then
		printf "%7s... OK\n" "$version"
	else
		echo "$output"
		printf "%7s... FAILED\n" "$version"
		rm -rf "$outdir"
		failed=1
	fi

	sudo rm -f "$outdir.tar.gz" "$outdir"
done


exit $failed
