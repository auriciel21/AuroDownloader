package org.aurodownloader.essai;

import android.app.Activity;
import android.content.ContentResolver;
import android.content.Intent;
import android.content.SharedPreferences;
import android.net.Uri;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.system.ErrnoException;
import android.system.Os;
import android.system.OsConstants;
import android.system.StructStat;
import android.util.Log;
import android.widget.Button;
import android.widget.LinearLayout;
import android.widget.ScrollView;
import android.widget.TextView;

import java.io.FileDescriptor;
import java.io.IOException;
import java.io.InputStream;
import java.io.OutputStream;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Random;

/**
 * Essai de faisabilité Android (SPEC 7.5 et 9.2) : un fichier obtenu par le sélecteur de
 * stockage accepte-t-il ce que fait le moteur ? Écritures positionnées depuis plusieurs
 * fils, dans le désordre ; vidage sur disque ; relecture ; puis reprise après la mort du
 * processus, par une permission persistante.
 *
 * Les appels passent par android.system.Os, qui fait les mêmes appels système que le
 * moteur (pwrite64, fdatasync, ftruncate) sur le même descripteur : le moteur recevrait
 * ce descripteur par ParcelFileDescriptor.detachFd(). Le journal va à l'écran et dans
 * logcat (étiquette AuroEssai).
 */
public class MainActivity extends Activity {
    private static final String TAG = "AuroEssai";
    private static final int PICK = 1;
    private static final int BLOCK = 1 << 20;
    private static final int BLOCKS = 64;
    /** Blocs laissés pour la reprise, écrits après la mort du processus. */
    private static final int LATER = 16;
    private static final int THREADS = 4;

    private TextView out;
    private SharedPreferences prefs;

    @Override
    protected void onCreate(Bundle state) {
        super.onCreate(state);
        prefs = getSharedPreferences("essai", MODE_PRIVATE);
        LinearLayout layout = new LinearLayout(this);
        layout.setOrientation(LinearLayout.VERTICAL);
        // Affichage bord à bord imposé dès Android 15 : rester hors des barres système.
        layout.setFitsSystemWindows(true);
        Button pick = new Button(this);
        pick.setText("1. Choisir un emplacement et tester");
        pick.setOnClickListener(v -> {
            Intent i = new Intent(Intent.ACTION_CREATE_DOCUMENT);
            i.addCategory(Intent.CATEGORY_OPENABLE);
            i.setType("application/octet-stream");
            i.putExtra(Intent.EXTRA_TITLE, "auro-essai.bin");
            startActivityForResult(i, PICK);
        });
        Button resume = new Button(this);
        resume.setText("2. Reprendre (après avoir tué l'application)");
        resume.setOnClickListener(v -> new Thread(this::resumeTest).start());
        out = new TextView(this);
        out.setTextIsSelectable(true);
        ScrollView scroll = new ScrollView(this);
        scroll.addView(out);
        layout.addView(pick);
        layout.addView(resume);
        layout.addView(scroll);
        setContentView(layout);
        log("Prêt. Dernier fichier : " + prefs.getString("uri", "aucun"));
    }

    @Override
    protected void onActivityResult(int request, int result, Intent data) {
        super.onActivityResult(request, result, data);
        if (request != PICK || result != RESULT_OK || data == null || data.getData() == null) {
            log("Aucun emplacement choisi.");
            return;
        }
        Uri uri = data.getData();
        try {
            getContentResolver().takePersistableUriPermission(uri,
                    Intent.FLAG_GRANT_READ_URI_PERMISSION | Intent.FLAG_GRANT_WRITE_URI_PERMISSION);
            log("Permission persistante : accordée");
        } catch (SecurityException e) {
            log("Permission persistante : REFUSÉE (" + e.getMessage() + ")");
        }
        prefs.edit().putString("uri", uri.toString()).apply();
        new Thread(() -> firstTest(uri)).start();
    }

    private void log(String line) {
        Log.i(TAG, line);
        runOnUiThread(() -> out.append(line + "\n"));
    }

    private static byte pattern(long pos) {
        return (byte) (pos ^ (pos >>> 8) ^ (pos >>> 16) ^ 0x5a);
    }

    private static byte[] block(int index) {
        byte[] b = new byte[BLOCK];
        long base = (long) index * BLOCK;
        for (int i = 0; i < BLOCK; i++) {
            b[i] = pattern(base + i);
        }
        return b;
    }

    private void firstTest(Uri uri) {
        log("---- Essai sur " + uri.getAuthority());
        ContentResolver cr = getContentResolver();
        try (ParcelFileDescriptor pfd = cr.openFileDescriptor(uri, "rw")) {
            FileDescriptor fd = pfd.getFileDescriptor();
            describe(pfd);
            try {
                Os.lseek(fd, 0, OsConstants.SEEK_SET);
                log("lseek : OK");
            } catch (ErrnoException e) {
                log("lseek : ÉCHEC " + e);
            }
            try {
                Os.ftruncate(fd, (long) BLOCK * BLOCKS);
                log("ftruncate à " + BLOCKS + " Mio : OK");
            } catch (ErrnoException e) {
                log("ftruncate : ÉCHEC " + e);
            }
            List<Integer> order = new ArrayList<>();
            for (int i = 0; i < BLOCKS - LATER; i++) {
                order.add(i);
            }
            Collections.shuffle(order, new Random(42));
            long t0 = System.nanoTime();
            String error = writeBlocks(fd, order);
            double seconds = (System.nanoTime() - t0) / 1e9;
            log(error == null
                    ? String.format("pwrite de %d Mio, %d fils, dans le désordre : OK (%.1f Mio/s)",
                    order.size(), THREADS, order.size() / seconds)
                    : "pwrite : ÉCHEC " + error);
            try {
                long t1 = System.nanoTime();
                Os.fdatasync(fd);
                log(String.format("fdatasync : OK (%.0f ms)", (System.nanoTime() - t1) / 1e6));
            } catch (ErrnoException e) {
                log("fdatasync : ÉCHEC " + e);
            }
            log("Relecture par pread : " + check(fd, 0, BLOCKS - LATER));
        } catch (IOException | RuntimeException e) {
            log("Ouverture en lecture-écriture : ÉCHEC " + e);
            fallbackTest(uri);
            return;
        }
        try (InputStream in = cr.openInputStream(uri)) {
            log("Relecture par flux après fermeture : " + checkStream(in, BLOCKS - LATER));
        } catch (IOException e) {
            log("Relecture par flux : ÉCHEC " + e);
        }
        log("Tuer maintenant l'application (adb shell am kill, ou depuis les applications "
                + "récentes), la relancer, puis toucher « Reprendre ».");
    }

    /**
     * Repli de la SPEC 7.5 quand l'écriture positionnée est impossible : le moteur
     * télécharge dans un fichier temporaire interne, puis le copie d'un seul flux.
     * L'essai fait cette copie finale, puis relit le fichier.
     */
    private void fallbackTest(Uri uri) {
        log("Repli : copie séquentielle par flux, comme celle d'un fichier temporaire interne");
        ContentResolver cr = getContentResolver();
        long t0 = System.nanoTime();
        try (OutputStream o = cr.openOutputStream(uri)) {
            if (o == null) {
                log("Flux en écriture : ÉCHEC (aucun flux)");
                return;
            }
            for (int i = 0; i < BLOCKS; i++) {
                o.write(block(i));
            }
        } catch (IOException | RuntimeException e) {
            log("Flux en écriture : ÉCHEC " + e);
            return;
        }
        double seconds = (System.nanoTime() - t0) / 1e9;
        log(String.format("Flux en écriture, %d Mio : OK (%.1f Mio/s)", BLOCKS, BLOCKS / seconds));
        try (InputStream in = cr.openInputStream(uri)) {
            log("Relecture par flux : " + checkStream(in, BLOCKS));
        } catch (IOException | RuntimeException e) {
            log("Relecture par flux : ÉCHEC " + e);
        }
    }

    private void resumeTest() {
        String saved = prefs.getString("uri", null);
        if (saved == null) {
            log("Aucun fichier à reprendre.");
            return;
        }
        Uri uri = Uri.parse(saved);
        log("---- Reprise de " + uri.getAuthority() + " (pid " + android.os.Process.myPid() + ")");
        try (ParcelFileDescriptor pfd = getContentResolver().openFileDescriptor(uri, "rw")) {
            FileDescriptor fd = pfd.getFileDescriptor();
            describe(pfd);
            log("Blocs écrits avant l'arrêt : " + check(fd, 0, BLOCKS - LATER));
            List<Integer> order = new ArrayList<>();
            for (int i = BLOCKS - 1; i >= BLOCKS - LATER; i--) {
                order.add(i);
            }
            String error = writeBlocks(fd, order);
            log(error == null ? "pwrite des " + LATER + " derniers Mio : OK" : "pwrite : ÉCHEC " + error);
            Os.fdatasync(fd);
            log("Fichier complet : " + check(fd, 0, BLOCKS)
                    + ", taille " + Os.fstat(fd).st_size + " octets");
        } catch (IOException | ErrnoException | RuntimeException e) {
            log("Reprise : ÉCHEC " + e);
        }
    }

    private void describe(ParcelFileDescriptor pfd) {
        try {
            StructStat st = Os.fstat(pfd.getFileDescriptor());
            String target = Os.readlink("/proc/self/fd/" + pfd.getFd());
            log("Descripteur " + pfd.getFd() + " → " + target
                    + (OsConstants.S_ISREG(st.st_mode) ? " (fichier ordinaire" : " (PAS un fichier ordinaire")
                    + ", " + st.st_size + " octets)");
        } catch (ErrnoException e) {
            log("fstat : ÉCHEC " + e);
        }
    }

    /** Écrit les blocs `order` à leur position, répartis entre THREADS fils. */
    private static String writeBlocks(FileDescriptor fd, List<Integer> order) {
        String[] error = new String[1];
        Thread[] threads = new Thread[THREADS];
        for (int t = 0; t < THREADS; t++) {
            final int first = t;
            threads[t] = new Thread(() -> {
                try {
                    for (int k = first; k < order.size(); k += THREADS) {
                        int index = order.get(k);
                        byte[] b = block(index);
                        long pos = (long) index * BLOCK;
                        int done = 0;
                        while (done < BLOCK) {
                            done += Os.pwrite(fd, b, done, BLOCK - done, pos + done);
                        }
                    }
                } catch (ErrnoException | java.io.InterruptedIOException e) {
                    synchronized (error) {
                        error[0] = e.toString();
                    }
                }
            });
            threads[t].start();
        }
        for (Thread th : threads) {
            try {
                th.join();
            } catch (InterruptedException e) {
                return e.toString();
            }
        }
        return error[0];
    }

    /** Vérifie les blocs [from, to) par pread. */
    private static String check(FileDescriptor fd, int from, int to) {
        byte[] b = new byte[BLOCK];
        try {
            for (int index = from; index < to; index++) {
                long pos = (long) index * BLOCK;
                int done = 0;
                while (done < BLOCK) {
                    int n = Os.pread(fd, b, done, BLOCK - done, pos + done);
                    if (n <= 0) {
                        return "ÉCHEC : fin de fichier au bloc " + index;
                    }
                    done += n;
                }
                for (int i = 0; i < BLOCK; i++) {
                    if (b[i] != pattern(pos + i)) {
                        return "ÉCHEC : octet faux à " + (pos + i);
                    }
                }
            }
        } catch (ErrnoException | java.io.InterruptedIOException e) {
            return "ÉCHEC " + e;
        }
        return "OK (" + (to - from) + " Mio)";
    }

    private static String checkStream(InputStream in, int blocks) throws IOException {
        byte[] b = new byte[BLOCK];
        for (int index = 0; index < blocks; index++) {
            int done = 0;
            while (done < BLOCK) {
                int n = in.read(b, done, BLOCK - done);
                if (n < 0) {
                    return "ÉCHEC : fin de flux au bloc " + index;
                }
                done += n;
            }
            long pos = (long) index * BLOCK;
            for (int i = 0; i < BLOCK; i++) {
                if (b[i] != pattern(pos + i)) {
                    return "ÉCHEC : octet faux à " + (pos + i);
                }
            }
        }
        return "OK (" + blocks + " Mio)";
    }
}
