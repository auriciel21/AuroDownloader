#include "engineclient.h"

#include <QCoreApplication>
#include <QJsonArray>
#include <QJsonDocument>

EngineClient::EngineClient(QString endpoint, QString destDir, QObject *parent)
    : QObject(parent), m_endpoint(std::move(endpoint)), m_destDir(std::move(destDir))
{
    connect(&m_socket, &QLocalSocket::readyRead, this, &EngineClient::onReadyRead);
    connect(&m_socket, &QLocalSocket::connected, this, [this] {
        m_connected = true;
        emit connectedChanged();
        const QJsonObject hello{
            {"client", QJsonObject{{"name", "auro-qt"}, {"version", "0.1"}}},
            {"protocol", QJsonObject{{"min", "1.0"}, {"max", "1.0"}}},
        };
        call("engine.hello", hello, [this](const QJsonObject &) {
            call("events.subscribe",
                 {{"topics", QJsonArray{"tasks", "progress"}}, {"progressIntervalMs", 500}},
                 [this](const QJsonObject &) {
                     call("sync.snapshot", {}, [this](const QJsonObject &snap) {
                         m_tasks.reset(snap.value("tasks").toArray());
                         emit snapshotLoaded();
                     });
                 });
        });
    });
    connect(&m_socket, &QLocalSocket::disconnected, this, [this] {
        m_connected = false;
        emit connectedChanged();
        setError(tr("Connexion au moteur perdue"));
    });
    connect(&m_socket, &QLocalSocket::errorOccurred, this, [this] {
        setError(tr("Moteur injoignable : %1").arg(m_socket.errorString()));
    });
    m_socket.connectToServer(m_endpoint);
}

void EngineClient::setError(const QString &text)
{
    m_lastError = text;
    emit lastErrorChanged();
}

void EngineClient::call(const QString &method, const QJsonObject &params, Reply onResult)
{
    const qint64 id = m_nextId++;
    const QJsonObject request{
        {"jsonrpc", "2.0"}, {"id", id}, {"method", method}, {"params", params}};
    QByteArray line = QJsonDocument(request).toJson(QJsonDocument::Compact);
    line.append('\n');
    m_waiting[id] = std::move(onResult);
    m_socket.write(line);
}

void EngineClient::onReadyRead()
{
    while (m_socket.canReadLine()) {
        const QByteArray line = m_socket.readLine();
        const QJsonDocument doc = QJsonDocument::fromJson(line);
        if (doc.isObject())
            onMessage(doc.object());
    }
}

void EngineClient::onMessage(const QJsonObject &message)
{
    if (message.contains("id")) {
        const qint64 id = message.value("id").toInteger();
        auto it = m_waiting.find(id);
        if (it == m_waiting.end())
            return;
        Reply reply = std::move(it->second);
        m_waiting.erase(it);
        if (message.contains("error")) {
            const QJsonObject error = message.value("error").toObject();
            setError(error.value("message").toString());
            return;
        }
        if (reply)
            reply(message.value("result").toObject());
        return;
    }
    const QString method = message.value("method").toString();
    const QJsonObject params = message.value("params").toObject();
    if (method == "task.progress")
        m_tasks.applyProgress(params.value("items").toArray());
    else if (method == "task.stateChanged")
        m_tasks.setState(params.value("taskId").toString(), params.value("to").toString());
    else if (method == "task.added")
        m_tasks.upsert(params.value("task").toObject());
}

void EngineClient::add(const QString &url)
{
    if (url.trimmed().isEmpty())
        return;
    call("task.add",
         {{"sources", QJsonArray{QJsonObject{{"url", url.trimmed()}}}}, {"destDir", m_destDir}});
}

void EngineClient::pause(const QString &taskId)
{
    call("task.pause", {{"taskIds", QJsonArray{taskId}}});
}

void EngineClient::resume(const QString &taskId)
{
    call("task.resume", {{"taskIds", QJsonArray{taskId}}});
}

void EngineClient::cancel(const QString &taskId)
{
    call("task.cancel", {{"taskIds", QJsonArray{taskId}}, {"deleteFiles", false}});
}
