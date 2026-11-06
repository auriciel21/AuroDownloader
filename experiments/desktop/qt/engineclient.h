// Client du protocole du moteur (SPEC §6) : un message JSON par ligne sur le point
// d'accès local (canal nommé sous Windows, socket Unix ailleurs, tous deux par QLocalSocket).
#pragma once

#include <QJsonObject>
#include <QLocalSocket>
#include <QObject>
#include <functional>
#include <map>

#include "taskmodel.h"

class EngineClient : public QObject {
    Q_OBJECT
    Q_PROPERTY(bool connected READ connected NOTIFY connectedChanged)
    Q_PROPERTY(QString lastError READ lastError NOTIFY lastErrorChanged)
    Q_PROPERTY(QObject *tasks READ tasksObject CONSTANT)

public:
    EngineClient(QString endpoint, QString destDir, QObject *parent = nullptr);

    bool connected() const { return m_connected; }
    QString lastError() const { return m_lastError; }
    TaskModel *tasks() { return &m_tasks; }
    QObject *tasksObject() { return &m_tasks; }

    Q_INVOKABLE void add(const QString &url);
    Q_INVOKABLE void pause(const QString &taskId);
    Q_INVOKABLE void resume(const QString &taskId);
    Q_INVOKABLE void cancel(const QString &taskId);

signals:
    void connectedChanged();
    void lastErrorChanged();
    void snapshotLoaded();

private:
    using Reply = std::function<void(const QJsonObject &result)>;
    void call(const QString &method, const QJsonObject &params, Reply onResult = {});
    void onReadyRead();
    void onMessage(const QJsonObject &message);
    void setError(const QString &text);

    QLocalSocket m_socket;
    QString m_endpoint;
    QString m_destDir;
    bool m_connected = false;
    QString m_lastError;
    qint64 m_nextId = 1;
    std::map<qint64, Reply> m_waiting;
    TaskModel m_tasks;
};
