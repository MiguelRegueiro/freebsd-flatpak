#ifndef STATUS_NOTIFIER_WATCHER_H
#define STATUS_NOTIFIER_WATCHER_H
#include <gio/gio.h>
#include <glib-unix.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
typedef struct _StatusNotifierBridge StatusNotifierBridge;
typedef struct _StatusItem StatusItem;
struct _StatusNotifierBridge {
  char *app_id;
  char *app_root;
  char *runtime_root;
  GMainLoop *loop;
  GDBusConnection *host_bus;
  GDBusConnection *local_bus;
  GDBusNodeInfo *watcher_node;
  GDBusNodeInfo *item_node;
  GDBusNodeInfo *dbusmenu_node;
  GPtrArray *status_items;
  guint64 status_counter;
  bool local_objects_registered;
};
extern const char *STATUS_WATCHER_XML;
extern const GDBusInterfaceVTable STATUS_WATCHER_VTABLE;
void status_notifier_log(const char *fmt, ...);
void status_notifier_diagnostic(const char *fmt, ...);
void free_status_item(StatusItem *);
#endif
