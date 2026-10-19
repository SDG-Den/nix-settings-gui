#!/usr/bin/env bash

files=$(find "$(pwd)/src" -type f -name "*.rs")
counter=0
count2=0
LIST=""

for file in $files; do
    lines=$(wc -l $file | cut -d' ' -f1)
    filepretty=$(echo "$file" | sed "s|$(pwd)||g")
    echo "$filepretty = $lines lines of code"
    counter=$(( $counter + $lines ))
    count2=$(( $count2 + 1))
    LIST=$(echo "$lines - $filepretty
$LIST")

done

LIST_SORTED=$(echo "$LIST" | sort -n)
echo ""
echo "lines sorted:"
echo "$LIST_SORTED"
echo ""
echo "total lines: $counter across $count2 files"
