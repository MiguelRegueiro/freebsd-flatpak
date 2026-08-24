#ifndef STATUS_NOTIFIER_ITEM_PROXY_H
#define STATUS_NOTIFIER_ITEM_PROXY_H
#include "status_notifier_watcher.h"
typedef struct _MenuProxy MenuProxy;
typedef struct {
  char *local_name;
  char *exposed_name;
  GVariant *pixmap;
  bool loaded;
} StatusIcon;
struct _StatusItem {
  StatusNotifierBridge *state;
  char *local_service;
  char *local_path;
  char *local_registration;
  char *host_path;
  guint host_registration_id;
  guint local_signal_id;
  GPtrArray *menus;
  StatusIcon icon;
  StatusIcon overlay_icon;
  StatusIcon attention_icon;
};
extern const char *STATUS_ITEM_XML;
extern const GDBusInterfaceVTable STATUS_ITEM_VTABLE;
void status_notifier_forward_call(GObject *, GAsyncResult *, gpointer);
bool register_host_status_item(StatusItem *, GError **);
bool register_with_host_watcher(StatusItem *, GError **);
void on_local_status_signal(GDBusConnection *, const gchar *, const gchar *,
                            const gchar *, const gchar *, GVariant *, gpointer);
#endif
