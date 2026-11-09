#!/bin/bash
# run.sh <rqbit|lt> <nom> [fraction à laquelle tuer le processus, ex. 0.4]
# PROTO_DIR : dossier (vu de WSL) qui contient netinst.torrent et bin/auro-torrent-rqbit.
T=${PROTO_DIR:?PROTO_DIR manquant}
tool=$1; name=$2; kill_at=$3
R=~/proto-torrent/runs/$name; rm -rf $R; mkdir -p $R/data $R/state
cp $T/netinst.torrent $R/
UP=102400
if [ $tool = rqbit ]; then
  cp $T/bin/auro-torrent-rqbit ~/proto-torrent/rqbit; chmod +x ~/proto-torrent/rqbit
  CMD=(~/proto-torrent/rqbit $R/netinst.torrent $R/data $R/state $UP)
else
  CMD=(~/proto-torrent/venv/bin/python -I /mnt/c/Dev/AuroDownloader/experiments/torrent/libtorrent/lt_proto.py $R/netinst.torrent $R/data $R/state $UP)
fi
phase() { /usr/bin/time -v -o $R/time$1.txt "${CMD[@]}" > $R/log$1.txt 2> $R/err$1.txt; }
summary() {
  echo "-- phase $1 : $(grep -E 'Elapsed|User time|System time|Maximum resident' $R/time$1.txt | sed -E 's/^\s+//' | tr '\n' ';')"
  grep -E "^FIN" $R/log$1.txt || tail -1 $R/log$1.txt
}
if [ -n "$kill_at" ]; then
  phase 1 &
  while sleep 1; do
    l=$(tail -1 $R/log1.txt 2>/dev/null); o=$(echo "$l" | grep -oE 'octets=[0-9]+' | cut -d= -f2); tot=$(echo "$l" | grep -oE 'total=[0-9]+' | cut -d= -f2)
    if [ -n "$tot" ] && [ "$tot" -gt 0 ] && [ $(( o * 100 / tot )) -ge $(echo "$kill_at*100/1" | bc) ]; then
      pkill -9 -f "$R/netinst.torrent" ; echo "tué à $o / $tot"; break; fi
  done
  wait
  if [ "$4" = corrupt ]; then f=$(find $R/data -type f | head -1); dd if=/dev/zero of="$f" bs=1M seek=64 count=4 conv=notrunc status=none; echo "effacé : 4 Mio à 64 Mio dans $(basename "$f")"; fi
  summary 1
  phase 2; summary 2
else
  phase 1; summary 1
fi
f=$(find $R/data -name '*.iso' | head -1); echo "fichier : $(stat -c %s "$f") octets"; sha256sum "$f" | cut -c1-64
echo "attendu  a7ef94ac2fb9a7fec454552abd629b7cc9d5155c886165a45649f5ce6167e355"
