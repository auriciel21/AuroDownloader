#!/bin/bash
# magnet.sh <rqbit|lt> <nom> : lien magnet sans tracker (DHT seul), même mesure que run.sh.
# PROTO_DIR : dossier (vu de WSL) qui contient netinst.torrent et bin/auro-torrent-rqbit.
T=${PROTO_DIR:?PROTO_DIR manquant}
H=$(~/proto-torrent/venv/bin/python -I -c "import libtorrent as lt; print(lt.torrent_info('$T/netinst.torrent').info_hashes().v1)")
M="magnet:?xt=urn:btih:$H&dn=debian-13.7.0-amd64-netinst.iso"
echo "$M"
R=~/proto-torrent/runs/$2; rm -rf $R; mkdir -p $R/data $R/state
if [ $1 = rqbit ]; then cp $T/bin/auro-torrent-rqbit ~/proto-torrent/rqbit; chmod +x ~/proto-torrent/rqbit; CMD=(~/proto-torrent/rqbit "$M" $R/data $R/state 102400)
else CMD=(~/proto-torrent/venv/bin/python -I /mnt/c/Dev/AuroDownloader/experiments/torrent/libtorrent/lt_proto.py "$M" $R/data $R/state 102400); fi
timeout 300 /usr/bin/time -v -o $R/time.txt "${CMD[@]}" > $R/log.txt 2> $R/err.txt; echo "rc=$?"
grep -E 'Elapsed|Maximum resident' $R/time.txt | sed -E 's/^\s+//' | tr '\n' ';'; echo
grep -m1 -E "total=[1-9]" $R/log.txt | grep -oE "^t=[0-9]+" | sed 's/^/métadonnées reçues à /'
grep FIN $R/log.txt || tail -2 $R/log.txt; tail -2 $R/err.txt
f=$(find $R/data -name '*.iso' | head -1); [ -n "$f" ] && sha256sum "$f" | cut -c1-64
