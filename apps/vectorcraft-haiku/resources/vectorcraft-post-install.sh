#!/bin/sh
# Registers VectorCraft's own document types and makes it the preferred application of those
# that have none yet; a preferred application the user chose stays.
signature=application/x-vnd.storytold-VectorCraft
app=$(findpaths -p "$0" B_FIND_PATH_APPS_DIRECTORY VectorCraft 2>/dev/null)
[ -n "$app" ] && [ -e "$app" ] && mimeset -a -f "$app"
setmime -set application/x-vectorcraft -short "VectorCraft document" -extension vectorcraft -extension drawcraft
setmime -set application/x-vectorcraft-template -short "VectorCraft template" -extension vctemplate
for type in application/x-vectorcraft application/x-vectorcraft-template application/illustrator image/x-eps; do
    setmime -dump "$type" 2>/dev/null | grep -q -- -preferredAppSig \
        || setmime -set "$type" -preferredAppSig "$signature"
done
exit 0
