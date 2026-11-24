#!/bin/sh
# Extra conflict fixtures for MG-01..26: one small repo per stopped state, under <out>/repos/extra.
#   scripts/flows/merge-fixture.sh <out>     (mkflows.py runs it; <out>/home must exist)
# merge-plain: merge style, 3 conflicts with merged regions, a CRLF file, a no-final-newline file, merge.guitool set to
#   a stub tool that sleeps 1 s before it copies theirs (so the "running" state shows); merge-diff3: diff3 style,
#   merge.tool=vimdiff (a terminal tool: Merge Tool disabled); rebase, cherry, revert: those operations stopped;
#   resolved: staged, waiting for Continue; outside: edited after the merge stopped; special: add/add, delete/modify both
#   ways, rename/delete, binary, symlink and submodule conflicts.
set -e
ROOT=$(cd "${1:?usage: merge-fixture.sh <out>}" && pwd -P)
OUT=$ROOT/repos/extra
G=/opt/homebrew/bin/git
export HOME=$ROOT/home GIT_CONFIG_NOSYSTEM=1
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE
export GIT_AUTHOR_DATE=2026-11-20T10:00:00 GIT_COMMITTER_DATE=2026-11-20T10:00:00
rm -rf "$OUT"; mkdir -p "$OUT"
c() { $G -C "$R" "$@"; }
new() { R=$OUT/$1; mkdir -p "$R"; c init -q -b main; }
base_text() {
  printf 'header\none\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\neleven\ntwelve\nthirteen\nfourteen\nfifteen\nsixteen\nfooter\n'
}
# The 3-conflict text file: ours changes "one" (clean), theirs "four" (clean), both "seven" alike; conflicts at
# two, ten (with matching added lines at the edges, so trimming applies) and fifteen.
ours_text() {
  printf 'header\nONE ours\ntwo ours\nthree\nfour\nfive\nsix\nSEVEN both\neight\nnine\nshared top\nten ours\nshared bottom\neleven\ntwelve\nthirteen\nfourteen\nfifteen ours\nsixteen\nfooter\n'
}
theirs_text() {
  printf 'header\none\ntwo theirs\nthree\nFOUR theirs\nfive\nsix\nSEVEN both\neight\nnine\nshared top\nten theirs\nshared bottom\neleven\ntwelve\nthirteen\nfourteen\nfifteen theirs\nsixteen\nfooter\n'
}
textmerge() { # name, style
  new "$1"
  [ -n "$2" ] && c config merge.conflictStyle "$2"
  base_text > "$R/notes.txt"; printf 'a\r\nb\r\nc\r\n' > "$R/crlf.txt"; printf 'x\ny' > "$R/nonl.txt"
  c add .; c commit -qm base
  c switch -qc topic
  theirs_text > "$R/notes.txt"; printf 'a\r\nB theirs\r\nc\r\n' > "$R/crlf.txt"; printf 'x\ny theirs' > "$R/nonl.txt"
  c commit -qam "topic: rewrite the notes"
  c switch -q main
  ours_text > "$R/notes.txt"; printf 'a\r\nB ours\r\nc\r\n' > "$R/crlf.txt"; printf 'x\ny ours' > "$R/nonl.txt"
  c commit -qam "main: rewrite the notes"
}
textmerge merge-plain ""
c merge topic >/dev/null 2>&1 || true
c config merge.guitool stubtool
c config mergetool.stubtool.cmd 'sleep 1; cp "$REMOTE" "$MERGED"'
c config mergetool.stubtool.trustExitCode false

textmerge merge-diff3 diff3
c merge topic >/dev/null 2>&1 || true
c config merge.tool vimdiff

textmerge rebase ""
c switch -q topic; c rebase main >/dev/null 2>&1 || true

textmerge cherry ""
c cherry-pick topic >/dev/null 2>&1 || true

# Revert: a later commit touches the reverted lines.
new revert
base_text > "$R/notes.txt"; c add .; c commit -qm base
sed -i '' 's/^two$/two v2/' "$R/notes.txt"; c commit -qam "notes: two v2"
sed -i '' 's/^two v2$/two v3/' "$R/notes.txt"; c commit -qam "notes: two v3"
c revert --no-edit HEAD~1 >/dev/null 2>&1 || true

# Resolved: a merge whose conflict is resolved and staged, waiting for Continue.
textmerge resolved ""
c merge topic >/dev/null 2>&1 || true
c checkout --theirs -- notes.txt crlf.txt nonl.txt; c add notes.txt crlf.txt nonl.txt

# Edited outside: text outside the conflicts changed after the merge stopped.
textmerge outside ""
c merge topic >/dev/null 2>&1 || true
sed -i '' 's/^header$/header edited in vim/' "$R/notes.txt"

# Special conflicts: both added, delete/modify both ways, added by us/them (rename/delete), binary, symlink, submodule.
new special
S=$OUT/sub; mkdir -p "$S"; $G -C "$S" init -q -b main; echo s > "$S/s"; $G -C "$S" add .; $G -C "$S" commit -qm s1
echo s2 > "$S/s"; $G -C "$S" commit -qam s2; echo s3 > "$S/s"; $G -C "$S" commit -qam s3
S1=$($G -C "$S" rev-parse HEAD~2); S2=$($G -C "$S" rev-parse HEAD~1); S3=$($G -C "$S" rev-parse HEAD)
echo keep > "$R/delus.txt"; echo keep > "$R/delthem.txt"; echo moved > "$R/renamed.txt"
printf '\x89PNG\r\n\x1a\n\0base' > "$R/logo.png"; ln -s target-base "$R/link"
c add .; c update-index --add --cacheinfo "160000,$S1,lib"; c commit -qm base
c switch -qc topic
printf 'flag: theirs\n' > "$R/both.yaml"; echo "changed on topic" > "$R/delus.txt"; c rm -q delthem.txt
c mv renamed.txt renamed-theirs.txt
printf '\x89PNG\r\n\x1a\n\0theirs-longer' > "$R/logo.png"; rm "$R/link"; ln -s target-theirs "$R/link"
c add -A; c update-index --add --cacheinfo "160000,$S3,lib"; c commit -qm "topic: specials"
c switch -q main
printf 'flag: ours\n' > "$R/both.yaml"; c rm -q delus.txt; echo "changed on main" > "$R/delthem.txt"
c mv renamed.txt renamed-ours.txt
printf '\x89PNG\r\n\x1a\n\0ours' > "$R/logo.png"; rm "$R/link"; ln -s target-ours "$R/link"
c add -A; c update-index --add --cacheinfo "160000,$S2,lib"; c commit -qm "main: specials"
c merge topic >/dev/null 2>&1 || true
c status --short >/dev/null
