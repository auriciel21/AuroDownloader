// Tâches affichées : une ligne par tâche, mise à jour par les notifications du moteur.
#pragma once

#include <QAbstractListModel>
#include <QJsonObject>
#include <QVector>

struct TaskRow {
    QString id;
    QString name;
    QString state;
    qint64 done = 0;
    qint64 total = -1; // -1 : taille inconnue
    qint64 bps = 0;
    qint64 etaS = -1;
    int connections = 0;
};

class TaskModel : public QAbstractListModel {
    Q_OBJECT
    Q_PROPERTY(int count READ count NOTIFY countChanged)

public:
    enum Role {
        IdRole = Qt::UserRole + 1,
        NameRole,
        StateRole,
        DoneRole,
        TotalRole,
        BpsRole,
        EtaRole,
        ConnectionsRole,
    };

    using QAbstractListModel::QAbstractListModel;

    int rowCount(const QModelIndex &parent = {}) const override;
    QVariant data(const QModelIndex &index, int role) const override;
    QHash<int, QByteArray> roleNames() const override;
    int count() const { return int(m_rows.size()); }

    void reset(const QJsonArray &tasks);
    void upsert(const QJsonObject &task);
    void setState(const QString &id, const QString &state);
    void applyProgress(const QJsonArray &items);

signals:
    void countChanged();

private:
    int indexOf(const QString &id) const;
    QVector<TaskRow> m_rows;
};
