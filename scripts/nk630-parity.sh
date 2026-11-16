#!/bin/bash
# nk630-parity.sh — 공식 통합명세서작성기(NK-Editor 6.30) 출력과 서안 출력을 바이트로 대조한다.
# 사용: scripts/nk630-parity.sh [추가 HLT 공식XML]...
# 기본 사례: fixtures/official-nk630 의 t2 · t3(공식 .hlz 안 XML → generate-hlt → export-fin --layout nk630).
set -u
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
B="$ROOT/target/release/hlt-cli"
T="$(mktemp -d "${TMPDIR:-/tmp}/nk630.XXXXXX")"
fail=0
mask() { sed 's/documentID="[0-9X]*"/documentID="X"/' "$1"; }
check() { # name hlt official-xml official-dir
  local n=$1 hlt=$2 off=$3 offdir=$4 pc
  pc=$(grep -o 'pageCount="[0-9]*"' "$off" | grep -o '[0-9]*')
  if ! "$B" export-fin "$hlt" "$T/$n.fin" --hlz-name "$n.hlz" --app-date 2026-11-16 --app-time 1 \
       --document-id X --page-count "$pc" --layout nk630 | grep -q '"ok":true'; then
    echo "FAIL $n: export-fin"; fail=1; return; fi
  mkdir -p "$T/$n"; (cd "$T/$n" && unzip -q "../$n.fin" && unzip -q "$n.hlz" -d h)
  if cmp -s <(mask "$off") <(mask "$T/$n/h/$n.xml"); then
    echo "OK   $n: XML byte-identical"
  else
    echo "FAIL $n: XML differs"; diff <(mask "$off" | cat -v) <(mask "$T/$n/h/$n.xml" | cat -v) | head -6; fail=1
  fi
  for img in "$offdir"/pat*; do
    [ -e "$img" ] || continue
    local mine="$T/$n/h/$(basename "$img")"
    if [ ! -e "$mine" ]; then echo "FAIL $n: missing $(basename "$img")"; fail=1; continue; fi
    if cmp -s <(magick "$img" -depth 1 gray:-) <(magick "$mine" -depth 1 gray:-); then
      echo "OK   $n: $(basename "$img") pixels identical ($(file -b "$mine" | grep -o 'group 4\|JPEG' | head -1))"
    else echo "FAIL $n: $(basename "$img") pixels differ"; fail=1; fi
  done
}
for n in t2 t3; do
  mkdir -p "$T/$n.src"; unzip -q -o "$ROOT/fixtures/official-nk630/$n.official.hlz" -d "$T/$n.src"
  X=$(ls "$T/$n.src"/*.xml)
  "$B" generate-hlt "$X" "$T/$n.HLT" --dialect patent-specification-ko | grep -q '"ok":true' || { echo "FAIL $n: generate-hlt"; fail=1; continue; }
  check "$n" "$T/$n.HLT" "$X" "$T/$n.src"
done
while [ $# -ge 2 ]; do check "extra$#" "$1" "$2" "$(dirname "$2")"; shift 2; done
echo "work: $T"
exit $fail
