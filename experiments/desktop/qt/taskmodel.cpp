#include "taskmodel.h"

#include <QJsonArray>

namespace {
TaskRow fromJson(const QJsonObject &t)
{
    TaskRow r;
    r.id = t.value("taskId").toString();
    r.name = t.value("displayName").toString();
    if (r.name.isEmpty())
        r.name = t.value("fileName").toString();
    r.state = t.value("state").toString();
    r.done = t.value("doneBytes").toInteger();
    r.total = t.value("totalBytes").isNull() ? -1 : t.value("totalBytes").toInteger();
    return r;
}
} // namespace

int TaskModel::rowCount(const QModelIndex &parent) const
{
    return parent.isValid() ? 0 : int(m_rows.size());
}

QVariant TaskModel::data(const QModelIndex &index, int role) const
{
    if (!index.isValid() || index.row() >= m_rows.size())
        return {};
    const TaskRow &r = m_rows.at(index.row());
    switch (role) {
    case IdRole: return r.id;
    case NameRole: return r.name;
    case StateRole: return r.state;
    case DoneRole: return double(r.done);
    case TotalRole: return double(r.total);
    case BpsRole: return double(r.bps);
    case EtaRole: return double(r.etaS);
    case ConnectionsRole: return r.connections;
    }
    return {};
}

QHash<int, QByteArray> TaskModel::roleNames() const
{
    return {
        {IdRole, "taskId"}, {NameRole, "name"}, {StateRole, "state"},
        {DoneRole, "done"}, {TotalRole, "total"}, {BpsRole, "bps"},
        {EtaRole, "eta"}, {ConnectionsRole, "connections"},
    };
}

int TaskModel::indexOf(const QString &id) const
{
    for (int i = 0; i < m_rows.size(); ++i)
        if (m_rows.at(i).id == id)
            return i;
    return -1;
}

void TaskModel::reset(const QJsonArray &tasks)
{
    beginResetModel();
    m_rows.clear();
    for (const auto &t : tasks)
        m_rows.append(fromJson(t.toObject()));
    endResetModel();
    emit countChanged();
}

void TaskModel::upsert(const QJsonObject &task)
{
    TaskRow row = fromJson(task);
    int i = indexOf(row.id);
    if (i < 0) {
        beginInsertRows({}, int(m_rows.size()), int(m_rows.size()));
        m_rows.append(row);
        endInsertRows();
        emit countChanged();
        return;
    }
    m_rows[i] = row;
    emit dataChanged(index(i), index(i));
}

void TaskModel::setState(const QString &id, const QString &state)
{
    int i = indexOf(id);
    if (i < 0)
        return;
    TaskRow &r = m_rows[i];
    r.state = state;
    if (state != "active") {
        r.bps = 0;
        r.etaS = -1;
        r.connections = 0;
    }
    if (state == "completed" && r.total >= 0)
        r.done = r.total;
    emit dataChanged(index(i), index(i));
}

void TaskModel::applyProgress(const QJsonArray &items)
{
    for (const auto &v : items) {
        const QJsonObject p = v.toObject();
        int i = indexOf(p.value("taskId").toString());
        if (i < 0)
            continue;
        TaskRow &r = m_rows[i];
        r.done = p.value("doneBytes").toInteger();
        if (!p.value("totalBytes").isNull())
            r.total = p.value("totalBytes").toInteger();
        r.bps = p.value("bps").toInteger();
        r.etaS = p.value("etaS").isNull() ? -1 : p.value("etaS").toInteger();
        r.connections = p.value("connections").toInt();
        emit dataChanged(index(i), index(i), {DoneRole, TotalRole, BpsRole, EtaRole, ConnectionsRole});
    }
}
