"""Prototype torrent avec libtorrent-rasterbar (phase 5, étape 2 ; experiments/torrent/README.md).

python lt_proto.py <fichier.torrent | lien magnet> <dossier> <état> <envoi max en octets/s>

Même rôle que le prototype librqbit : télécharge le torrent dans <dossier>, reprend où un
lancement précédent s'est arrêté (données de reprise dans <état>, enregistrées toutes les
5 s comme le ferait une vraie intégration, écrites puis renommées), envoi limité, sans
UPnP, NAT-PMP ni découverte sur le réseau local. Une ligne par seconde, puis arrêt dès la
fin du téléchargement, sans continuer à partager. Tout le travail se fait dans le code C++
de la bibliothèque ; Python ne fait que la piloter.
"""

import os
import sys
import time

import libtorrent as lt


def save(path, params):
    tmp = path + ".tmp"
    with open(tmp, "wb") as f:
        f.write(lt.write_resume_data_buf(params))
        f.flush()
        os.fsync(f.fileno())
    os.replace(tmp, path)


def main():
    torrent, folder, state, upload = sys.argv[1:5]
    os.makedirs(state, exist_ok=True)
    os.makedirs(folder, exist_ok=True)
    started = time.monotonic()
    ses = lt.session({
        "user_agent": "auro-prototype 0.1",
        "upload_rate_limit": int(upload),
        # Sinon, le surcoût IP de la réception (accusés de réception TCP) compte dans la
        # limite d'envoi : à 100 Kio/s, il freine le téléchargement lui-même (2 à 4 Mo/s
        # au premier essai, contre 58 Mio/s pour librqbit sur le même essaim).
        "rate_limit_ip_overhead": False,
        "enable_upnp": False,
        "enable_natpmp": False,
        "enable_lsd": False,
        "alert_mask": lt.alert_category.status | lt.alert_category.storage | lt.alert_category.error,
    })
    resume = os.path.join(state, "resume.dat")
    # Un lien magnet, ou le chemin d'un fichier .torrent.
    if os.path.exists(resume):
        with open(resume, "rb") as f:
            atp = lt.read_resume_data(f.read())
    elif torrent.startswith("magnet:"):
        atp = lt.parse_magnet_uri(torrent)
    else:
        atp = lt.add_torrent_params()
    if not torrent.startswith("magnet:"):
        atp.ti = lt.torrent_info(torrent)
    atp.save_path = folder
    h = ses.add_torrent(atp)
    print(f"ajouté en {time.monotonic() - started:.1f} s", file=sys.stderr)

    resumed_at = None
    first_byte = None
    last_save = time.monotonic()
    while True:
        s = h.status()
        t = time.monotonic() - started
        if resumed_at is None and s.state == lt.torrent_status.downloading:
            resumed_at = s.total_wanted_done
        if first_byte is None and resumed_at is not None and s.total_wanted_done > resumed_at:
            first_byte = t
        print(f"t={t:.0f} etat={s.state} octets={s.total_wanted_done} total={s.total_wanted} "
              f"recu={s.download_payload_rate / 1e6:.2f} envoye={s.total_payload_upload} "
              f"pairs={s.num_peers}", flush=True)
        for a in ses.pop_alerts():
            if isinstance(a, lt.save_resume_data_alert):
                save(resume, a.params)
            elif isinstance(a, (lt.torrent_error_alert, lt.file_error_alert)):
                print(f"erreur : {a.message()}", file=sys.stderr)
        if s.is_finished or s.state == lt.torrent_status.seeding:
            print(f"FIN t={t:.1f} repris_a={resumed_at or 0} premier_octet={first_byte or 0:.1f} "
                  f"envoye={s.total_payload_upload}", flush=True)
            break
        if time.monotonic() - last_save >= 5:
            h.save_resume_data(lt.torrent_handle.flush_disk_cache)
            last_save = time.monotonic()
        time.sleep(1)
    # Arrêt sans partager : torrent retiré de la session, fichiers gardés.
    ses.remove_torrent(h)


if __name__ == "__main__":
    main()
