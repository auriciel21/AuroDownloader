// Prototype Qt Quick de l'application de bureau (experiments/desktop/README.md).
#include <QAction>
#include <QApplication>
#include <QCommandLineParser>
#include <QDateTime>
#include <QIcon>
#include <QLocale>
#include <QMenu>
#include <QPainter>
#include <QPixmap>
#include <QQmlApplicationEngine>
#include <QQuickWindow>
#include <QSystemTrayIcon>
#include <QTranslator>
#include <cstdio>

#include "engineclient.h"

namespace {
QIcon appIcon()
{
    QPixmap pixmap(32, 32);
    pixmap.fill(Qt::transparent);
    QPainter p(&pixmap);
    p.setRenderHint(QPainter::Antialiasing);
    p.setBrush(QColor(0x2b, 0x6c, 0xb0));
    p.setPen(Qt::NoPen);
    p.drawEllipse(2, 2, 28, 28);
    p.setBrush(Qt::white);
    const QPointF arrow[] = {{10, 13}, {22, 13}, {16, 23}};
    p.drawPolygon(arrow, 3);
    p.drawRect(14, 6, 4, 8);
    return QIcon(pixmap);
}
} // namespace

int main(int argc, char *argv[])
{
    QApplication app(argc, argv);
    app.setApplicationName("AuroDownloader");
    app.setQuitOnLastWindowClosed(false);
    app.setWindowIcon(appIcon());

    QCommandLineParser parser;
    parser.addOptions({
        {"endpoint", "Point d'accès du moteur.", "endpoint"},
        {"dest", "Dossier des téléchargements ajoutés.", "dir"},
        {"lang", "Langue (fr, en).", "lang"},
    });
    parser.process(app);

    QTranslator translator;
    const QString lang = parser.value("lang");
    const bool english = lang == "en" || (lang.isEmpty() && QLocale().language() != QLocale::French);
    if (english && translator.load(":/i18n/auro_en.qm"))
        app.installTranslator(&translator);

    EngineClient client(parser.value("endpoint"), parser.value("dest"));

    QQmlApplicationEngine engine;
    engine.setInitialProperties({{"client", QVariant::fromValue<QObject *>(&client)}});
    engine.loadFromModule("Auro", "Main");
    if (engine.rootObjects().isEmpty())
        return 1;
    auto *window = qobject_cast<QQuickWindow *>(engine.rootObjects().constFirst());

    // Première image affichée avec les tâches chargées : mesure du démarrage.
    QObject::connect(&client, &EngineClient::snapshotLoaded, window, [window] {
        auto *once = new QMetaObject::Connection;
        *once = QObject::connect(window, &QQuickWindow::frameSwapped, window, [once] {
            QObject::disconnect(*once);
            delete once;
            std::printf("PREMIERE_IMAGE %lld\n", QDateTime::currentMSecsSinceEpoch());
            std::fflush(stdout);
        });
        window->update();
    }, Qt::SingleShotConnection);

    QSystemTrayIcon tray(appIcon());
    tray.setToolTip("AuroDownloader");
    QMenu menu;
    QAction *open = menu.addAction(QCoreApplication::translate("main", "Ouvrir"));
    QAction *quit = menu.addAction(QCoreApplication::translate("main", "Quitter"));
    QObject::connect(open, &QAction::triggered, window, [window] {
        window->show();
        window->raise();
        window->requestActivate();
    });
    QObject::connect(quit, &QAction::triggered, &app, &QApplication::quit);
    QObject::connect(&tray, &QSystemTrayIcon::activated, window, [window](auto reason) {
        if (reason == QSystemTrayIcon::Trigger) {
            window->show();
            window->raise();
            window->requestActivate();
        }
    });
    tray.setContextMenu(&menu);
    tray.show();

    return app.exec();
}
