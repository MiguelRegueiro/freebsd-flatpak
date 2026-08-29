#ifndef FREEBSD_FLATPAK_PORTAL_BRIDGE_PROCESS_H
#define FREEBSD_FLATPAK_PORTAL_BRIDGE_PROCESS_H
#include <errno.h>
#include <fcntl.h>
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <glib/gstdio.h>
#include <limits.h>
#include <pipewire/pipewire.h>
#include <signal.h>
#include <spa/utils/result.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/user.h>
#include <sys/wait.h>
#include <unistd.h>
#include "host_command.h"
typedef struct _PortalBridgeProcess PortalBridgeProcess;
typedef PortalBridgeProcess BridgeState;
typedef struct _DocumentGrant DocumentGrant;
typedef struct _RequestRecord RequestRecord;
typedef struct _SessionRecord SessionRecord;
typedef struct _PipeWireCompat PipeWireCompat;
typedef enum {
  REQUEST_FILECHOOSER,
  REQUEST_OPEN_URI,
  REQUEST_SCREENCAST_CREATE,
  REQUEST_SCREENCAST_OTHER,
  REQUEST_SCREENCAST_START
} RequestKind;
typedef struct {
  uint32_t node_id;
  uint64_t serial;
} ScreenCastSource;
typedef struct {
  char *doc_dir;
  char *sandbox_root;
  char *mountpoint;
  char *persistent_store;
  GPtrArray *sandbox_doc_dirs;
  GPtrArray *grants;
} DocumentGrantStore;
typedef struct {
  GPtrArray *requests;
  guint64 request_counter;
  guint64 host_token_counter;
} PortalRequestStore;
typedef struct {
  GPtrArray *sessions;
  PipeWireCompat *pipewire;
  guint32 version;
  guint32 source_types;
  guint32 cursor_modes;
} ScreenCastPortalState;
typedef struct {
  guint32 version;
} OpenUriPortalState;
struct _DocumentGrant {
  char *doc_id;
  char *host_path;
  char *placeholder_path;
  GPtrArray *target_paths;
  char *app_id;
  char **permissions;
  bool is_directory;
  bool persistent;
};
struct _RequestRecord {
  BridgeState *state;
  char *client_sender;
  char *local_path;
  char *host_path;
  char *local_session_path;
  guint local_registration_id;
  guint host_signal_id;
  RequestKind kind;
  SessionRecord *session;
  bool completed;
  bool close_requested;
  bool filechooser_directory;
};
struct _SessionRecord {
  BridgeState *state;
  char *client_sender;
  char *local_path;
  char *host_path;
  guint local_registration_id;
  guint host_signal_id;
  GArray *sources;
  bool close_requested;
  bool closed;
};
struct _PortalBridgeProcess {
  char *app_id;
  GMainLoop *loop;
  GDBusConnection *host_bus;
  GDBusConnection *local_bus;
  GDBusNodeInfo *desktop_node;
  GDBusNodeInfo *documents_node;
  GDBusNodeInfo *request_node;
  GDBusNodeInfo *session_node;
  GDBusNodeInfo *control_node;
  GDBusNodeInfo *flatpak_node;
  HostCommandService host_command;
  bool enable_host_command;
  DocumentGrantStore documents;
  PortalRequestStore request_store;
  OpenUriPortalState open_uri;
  ScreenCastPortalState screencast;
  guint local_name_signal_id;
  bool local_objects_registered;
  GPtrArray *spawn_lifecycles;
};
void log_line(const char *fmt, ...);
void diagnostic_line(const char *fmt, ...);
void portal_bridge_process_load_host_properties(BridgeState *state);
void portal_bridge_process_cleanup_documents(BridgeState *state);
gboolean portal_bridge_process_handle_signal(gpointer user_data);
void portal_bridge_process_on_bus_acquired(GDBusConnection *, const gchar *,
                                           gpointer);
void portal_bridge_process_on_name_acquired(GDBusConnection *, const gchar *,
                                            gpointer);
void portal_bridge_process_on_name_lost(GDBusConnection *, const gchar *,
                                        gpointer);
#endif
