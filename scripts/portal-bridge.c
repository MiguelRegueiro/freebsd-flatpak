#include <errno.h>
#include <fcntl.h>
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <glib/gstdio.h>
#include <limits.h>
#include <pipewire/pipewire.h>
#include <spa/utils/result.h>
#include <signal.h>
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

static const char *DESKTOP_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.FileChooser'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='OpenFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFile'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SaveFiles'>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='s' name='title' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Settings'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Read'>"
    "      <arg type='s' name='namespace' direction='in'/>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='v' name='value' direction='out'/>"
    "    </method>"
    "    <method name='ReadAll'>"
    "      <arg type='as' name='namespaces' direction='in'/>"
    "      <arg type='a{sa{sv}}' name='values' direction='out'/>"
    "    </method>"
    "    <signal name='SettingChanged'>"
    "      <arg type='s' name='namespace'/>"
    "      <arg type='s' name='key'/>"
    "      <arg type='v' name='value'/>"
    "    </signal>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.ProxyResolver'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Lookup'>"
    "      <arg type='s' name='uri' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='as' name='proxies' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.Inhibit'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Inhibit'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='CreateMonitor'>"
    "      <arg type='s' name='window' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='QueryEndResponse'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "    </method>"
    "    <signal name='StateChanged'>"
    "      <arg type='o' name='session_handle'/>"
    "      <arg type='a{sv}' name='state'/>"
    "    </signal>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.ScreenCast'>"
    "    <property name='AvailableSourceTypes' type='u' access='read'/>"
    "    <property name='AvailableCursorModes' type='u' access='read'/>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='CreateSession'>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='SelectSources'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='Start'>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='s' name='parent_window' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='o' name='handle' direction='out'/>"
    "    </method>"
    "    <method name='OpenPipeWireRemote'>"
    "      <annotation name='org.gtk.GDBus.C.UnixFD' value='true'/>"
    "      <arg type='o' name='session_handle' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='h' name='fd' direction='out'/>"
    "    </method>"
    "  </interface>"
    "</node>";

static const char *DOCUMENTS_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Documents'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='GetMountPoint'>"
    "      <arg type='ay' name='path' direction='out'/>"
    "    </method>"
    "    <method name='Add'>"
    "      <arg type='h' name='o_path_fd' direction='in'/>"
    "      <arg type='b' name='reuse_existing' direction='in'/>"
    "      <arg type='b' name='persistent' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='AddNamed'>"
    "      <arg type='h' name='o_path_parent_fd' direction='in'/>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='b' name='reuse_existing' direction='in'/>"
    "      <arg type='b' name='persistent' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='AddFull'>"
    "      <arg type='ah' name='o_path_fds' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "      <arg type='as' name='doc_ids' direction='out'/>"
    "      <arg type='a{sv}' name='extra_out' direction='out'/>"
    "    </method>"
    "    <method name='AddNamedFull'>"
    "      <arg type='h' name='o_path_fd' direction='in'/>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='u' name='flags' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "      <arg type='a{sv}' name='extra_out' direction='out'/>"
    "    </method>"
    "    <method name='GrantPermissions'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "    </method>"
    "    <method name='RevokePermissions'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='as' name='permissions' direction='in'/>"
    "    </method>"
    "    <method name='Delete'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "    </method>"
    "    <method name='Lookup'>"
    "      <arg type='ay' name='filename' direction='in'/>"
    "      <arg type='s' name='doc_id' direction='out'/>"
    "    </method>"
    "    <method name='Info'>"
    "      <arg type='s' name='doc_id' direction='in'/>"
    "      <arg type='ay' name='path' direction='out'/>"
    "      <arg type='a{sas}' name='apps' direction='out'/>"
    "    </method>"
    "    <method name='List'>"
    "      <arg type='s' name='app_id' direction='in'/>"
    "      <arg type='a{say}' name='docs' direction='out'/>"
    "    </method>"
    "    <method name='GetHostPaths'>"
    "      <arg type='as' name='doc_ids' direction='in'/>"
    "      <arg type='a{say}' name='paths' direction='out'/>"
    "    </method>"
    "  </interface>"
    "  <interface name='org.freedesktop.portal.FileTransfer'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='StartTransfer'>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='s' name='key' direction='out'/>"
    "    </method>"
    "    <method name='AddFiles'>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='ah' name='fds' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "    </method>"
    "    <method name='RetrieveFiles'>"
    "      <arg type='s' name='key' direction='in'/>"
    "      <arg type='a{sv}' name='options' direction='in'/>"
    "      <arg type='as' name='files' direction='out'/>"
    "    </method>"
    "    <method name='StopTransfer'>"
    "      <arg type='s' name='key' direction='in'/>"
    "    </method>"
    "  </interface>"
    "</node>";

static const char *REQUEST_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Request'>"
    "    <method name='Close'/>"
    "    <signal name='Response'>"
    "      <arg type='u' name='response'/>"
    "      <arg type='a{sv}' name='results'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

static const char *SESSION_XML =
    "<node>"
    "  <interface name='org.freedesktop.portal.Session'>"
    "    <property name='version' type='u' access='read'/>"
    "    <method name='Close'/>"
    "    <signal name='Closed'>"
    "      <arg type='a{sv}' name='details'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

static const char *STATUS_WATCHER_XML =
    "<node>"
    "  <interface name='org.kde.StatusNotifierWatcher'>"
    "    <property name='RegisteredStatusNotifierItems' type='as' access='read'/>"
    "    <property name='IsStatusNotifierHostRegistered' type='b' access='read'/>"
    "    <property name='ProtocolVersion' type='i' access='read'/>"
    "    <method name='RegisterStatusNotifierItem'>"
    "      <arg type='s' name='service' direction='in'/>"
    "    </method>"
    "    <method name='RegisterStatusNotifierHost'>"
    "      <arg type='s' name='service' direction='in'/>"
    "    </method>"
    "    <signal name='StatusNotifierItemRegistered'>"
    "      <arg type='s' name='service'/>"
    "    </signal>"
    "    <signal name='StatusNotifierItemUnregistered'>"
    "      <arg type='s' name='service'/>"
    "    </signal>"
    "    <signal name='StatusNotifierHostRegistered'/>"
    "    <signal name='StatusNotifierHostUnregistered'/>"
    "  </interface>"
    "</node>";

static const char *STATUS_ITEM_XML =
    "<node>"
    "  <interface name='org.kde.StatusNotifierItem'>"
    "    <property name='Category' type='s' access='read'/>"
    "    <property name='Id' type='s' access='read'/>"
    "    <property name='Title' type='s' access='read'/>"
    "    <property name='Status' type='s' access='read'/>"
    "    <property name='WindowId' type='u' access='read'/>"
    "    <property name='IconName' type='s' access='read'/>"
    "    <property name='IconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='OverlayIconName' type='s' access='read'/>"
    "    <property name='OverlayIconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='AttentionIconName' type='s' access='read'/>"
    "    <property name='AttentionIconPixmap' type='a(iiay)' access='read'/>"
    "    <property name='AttentionMovieName' type='s' access='read'/>"
    "    <property name='ToolTip' type='(sa(iiay)ss)' access='read'/>"
    "    <property name='ItemIsMenu' type='b' access='read'/>"
    "    <property name='Menu' type='o' access='read'/>"
    "    <method name='ContextMenu'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='Activate'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='SecondaryActivate'>"
    "      <arg type='i' name='x' direction='in'/>"
    "      <arg type='i' name='y' direction='in'/>"
    "    </method>"
    "    <method name='Scroll'>"
    "      <arg type='i' name='delta' direction='in'/>"
    "      <arg type='s' name='orientation' direction='in'/>"
    "    </method>"
    "    <signal name='NewTitle'/>"
    "    <signal name='NewIcon'/>"
    "    <signal name='NewAttentionIcon'/>"
    "    <signal name='NewOverlayIcon'/>"
    "    <signal name='NewToolTip'/>"
    "    <signal name='NewStatus'>"
    "      <arg type='s' name='status'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

static const char *DBUSMENU_XML =
    "<node>"
    "  <interface name='com.canonical.dbusmenu'>"
    "    <method name='GetLayout'>"
    "      <arg type='i' name='parentId' direction='in'/>"
    "      <arg type='i' name='recursionDepth' direction='in'/>"
    "      <arg type='as' name='propertyNames' direction='in'/>"
    "      <arg type='u' name='revision' direction='out'/>"
    "      <arg type='(ia{sv}av)' name='layout' direction='out'/>"
    "    </method>"
    "    <method name='GetGroupProperties'>"
    "      <arg type='ai' name='ids' direction='in'/>"
    "      <arg type='as' name='propertyNames' direction='in'/>"
    "      <arg type='a(ia{sv})' name='properties' direction='out'/>"
    "    </method>"
    "    <method name='GetProperty'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='s' name='name' direction='in'/>"
    "      <arg type='v' name='value' direction='out'/>"
    "    </method>"
    "    <method name='Event'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='s' name='eventId' direction='in'/>"
    "      <arg type='v' name='data' direction='in'/>"
    "      <arg type='u' name='timestamp' direction='in'/>"
    "    </method>"
    "    <method name='EventGroup'>"
    "      <arg type='a(isvu)' name='events' direction='in'/>"
    "      <arg type='ai' name='idErrors' direction='out'/>"
    "    </method>"
    "    <method name='AboutToShow'>"
    "      <arg type='i' name='id' direction='in'/>"
    "      <arg type='b' name='needUpdate' direction='out'/>"
    "    </method>"
    "    <method name='AboutToShowGroup'>"
    "      <arg type='ai' name='ids' direction='in'/>"
    "      <arg type='ai' name='updatesNeeded' direction='out'/>"
    "      <arg type='ai' name='idErrors' direction='out'/>"
    "    </method>"
    "    <signal name='ItemsPropertiesUpdated'>"
    "      <arg type='a(ia{sv})' name='updatedProps'/>"
    "      <arg type='a(ias)' name='removedProps'/>"
    "    </signal>"
    "    <signal name='LayoutUpdated'>"
    "      <arg type='u' name='revision'/>"
    "      <arg type='i' name='parent'/>"
    "    </signal>"
    "  </interface>"
    "</node>";

static const char *CONTROL_XML =
    "<node>"
    "  <interface name='org.freebsd.Flatpak.PortalBridge'>"
    "    <method name='AddSandbox'>"
    "      <arg type='s' name='sandbox_doc_dir' direction='in'/>"
    "    </method>"
    "    <method name='RemoveSandbox'>"
    "      <arg type='s' name='sandbox_doc_dir' direction='in'/>"
    "    </method>"
    "  </interface>"
    "</node>";

typedef struct {
    char *doc_id;
    char *host_path;
    char *placeholder_path;
    GPtrArray *target_paths;
    char *app_id;
    char **permissions;
} DocumentGrant;

typedef struct _BridgeState BridgeState;
typedef struct _StatusItem StatusItem;
typedef struct _SessionRecord SessionRecord;
typedef struct _PipeWireCompat PipeWireCompat;

typedef enum {
    REQUEST_FILECHOOSER,
    REQUEST_SCREENCAST_CREATE,
    REQUEST_SCREENCAST_OTHER,
    REQUEST_SCREENCAST_START,
} RequestKind;

typedef struct {
    uint32_t node_id;
    uint64_t serial;
} ScreenCastSource;

typedef struct {
    StatusItem *item;
    char *local_path;
    char *host_path;
    guint host_registration_id;
    guint local_signal_id;
} MenuProxy;

struct _StatusItem {
    BridgeState *state;
    char *local_service;
    char *local_path;
    char *local_registration;
    char *host_path;
    guint host_registration_id;
    guint local_signal_id;
    GPtrArray *menus;
};

typedef struct {
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
} RequestRecord;

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

struct _BridgeState {
    char *app_id;
    char *doc_dir;
    char *sandbox_root;
    char *mountpoint;
    GPtrArray *sandbox_doc_dirs;
    GPtrArray *grants;
    GPtrArray *requests;
    GPtrArray *sessions;
    GPtrArray *status_items;
    guint64 counter;
    guint64 request_counter;
    guint64 host_token_counter;
    guint64 status_counter;
    GMainLoop *loop;
    GDBusConnection *host_bus;
    GDBusConnection *local_bus;
    GDBusNodeInfo *desktop_node;
    GDBusNodeInfo *documents_node;
    GDBusNodeInfo *request_node;
    GDBusNodeInfo *session_node;
    GDBusNodeInfo *status_watcher_node;
    GDBusNodeInfo *status_item_node;
    GDBusNodeInfo *dbusmenu_node;
    GDBusNodeInfo *control_node;
    PipeWireCompat *pipewire;
    guint local_name_signal_id;
    guint32 screencast_version;
    guint32 screencast_source_types;
    guint32 screencast_cursor_modes;
    bool local_objects_registered;
};

static void log_line(const char *fmt, ...)
{
    va_list ap;
    va_start(ap, fmt);
    fputs("portal bridge: ", stderr);
    vfprintf(stderr, fmt, ap);
    fputc('\n', stderr);
    va_end(ap);
}

typedef struct {
    PipeWireCompat *compat;
    uint32_t id;
    struct pw_client *proxy;
    struct spa_hook listener;
    GArray *permissions;
    bool is_portal;
    bool permissions_received;
} PipeWireClient;

typedef struct {
    uint32_t id;
    uint32_t client_id;
    uint64_t serial;
    char *media_class;
    char *target_object;
} PipeWireNode;

typedef struct {
    uint32_t id;
    uint32_t node_id;
    bool is_input;
    bool is_output;
} PipeWirePort;

typedef struct {
    PipeWireCompat *compat;
    SessionRecord *session;
    struct pw_proxy *proxy;
    struct spa_hook proxy_listener;
    uint32_t source_node_id;
    uint32_t source_port_id;
    uint32_t consumer_client_id;
    uint32_t consumer_node_id;
    uint32_t consumer_port_id;
} PipeWireLink;

typedef struct {
    GSource source;
    PipeWireCompat *compat;
} PipeWireSource;

struct _PipeWireCompat {
    BridgeState *state;
    struct pw_main_loop *loop;
    struct pw_context *context;
    struct pw_core *core;
    struct pw_registry *registry;
    struct spa_hook core_listener;
    struct spa_hook registry_listener;
    GSource *source;
    GPtrArray *clients;
    GPtrArray *nodes;
    GPtrArray *ports;
    GPtrArray *links;
};

static void pipewire_compat_try_links(PipeWireCompat *compat);

static uint32_t parse_pipewire_id(const char *value)
{
    if (value == NULL || *value == '\0') {
        return SPA_ID_INVALID;
    }
    char *end = NULL;
    errno = 0;
    unsigned long parsed = strtoul(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0' || parsed > UINT32_MAX) {
        return SPA_ID_INVALID;
    }
    return (uint32_t)parsed;
}

static uint64_t parse_pipewire_serial(const char *value)
{
    if (value == NULL || *value == '\0') {
        return 0;
    }
    char *end = NULL;
    errno = 0;
    unsigned long long parsed = strtoull(value, &end, 10);
    if (errno != 0 || end == value || *end != '\0') {
        return 0;
    }
    return (uint64_t)parsed;
}

static ScreenCastSource *session_source_for_id(SessionRecord *session,
                                                uint32_t node_id)
{
    if (session == NULL || session->sources == NULL) {
        return NULL;
    }
    for (guint i = 0; i < session->sources->len; i++) {
        ScreenCastSource *source = &g_array_index(session->sources,
                                                  ScreenCastSource, i);
        if (source->node_id == node_id) {
            return source;
        }
    }
    return NULL;
}

static bool session_approves_source(SessionRecord *session, uint32_t node_id)
{
    return session_source_for_id(session, node_id) != NULL;
}

static bool source_generation_matches(const ScreenCastSource *source,
                                      const PipeWireNode *node)
{
    return source != NULL && node != NULL && source->node_id == node->id &&
           (source->serial == 0 || node->serial == 0 ||
            source->serial == node->serial);
}

static void remove_session_source_for_node(SessionRecord *session,
                                           const PipeWireNode *node)
{
    if (session == NULL || session->sources == NULL || node == NULL) {
        return;
    }
    for (guint i = session->sources->len; i > 0; i--) {
        ScreenCastSource *source = &g_array_index(session->sources,
                                                  ScreenCastSource, i - 1);
        if (source_generation_matches(source, node)) {
            g_array_remove_index(session->sources, i - 1);
        }
    }
}

static void free_pipewire_client(PipeWireClient *client)
{
    if (client == NULL) {
        return;
    }
    if (client->proxy != NULL) {
        spa_hook_remove(&client->listener);
        pw_proxy_destroy((struct pw_proxy *)client->proxy);
    }
    if (client->permissions != NULL) {
        g_array_free(client->permissions, TRUE);
    }
    g_free(client);
}

static void free_pipewire_node(PipeWireNode *node)
{
    if (node == NULL) {
        return;
    }
    g_free(node->media_class);
    g_free(node->target_object);
    g_free(node);
}

static void free_pipewire_port(PipeWirePort *port)
{
    g_free(port);
}

static void free_pipewire_link(PipeWireLink *link)
{
    if (link == NULL) {
        return;
    }
    if (link->proxy != NULL) {
        spa_hook_remove(&link->proxy_listener);
        pw_proxy_destroy(link->proxy);
    }
    g_free(link);
}

static PipeWireNode *find_pipewire_node(PipeWireCompat *compat, uint32_t id)
{
    for (guint i = 0; i < compat->nodes->len; i++) {
        PipeWireNode *node = g_ptr_array_index(compat->nodes, i);
        if (node->id == id) {
            return node;
        }
    }
    return NULL;
}

static bool pipewire_client_permission(PipeWireClient *client,
                                       uint32_t object_id,
                                       uint32_t *out_permissions)
{
    for (guint i = 0; i < client->permissions->len; i++) {
        struct pw_permission *permission =
            &g_array_index(client->permissions, struct pw_permission, i);
        if (permission->id == object_id) {
            *out_permissions = permission->permissions;
            return true;
        }
    }
    *out_permissions = 0;
    return false;
}

static bool pipewire_client_is_restricted(PipeWireClient *client)
{
    uint32_t default_permissions = 0;
    return client->is_portal && client->permissions_received &&
           pipewire_client_permission(client, PW_ID_ANY,
                                      &default_permissions) &&
           default_permissions == 0;
}

static bool pipewire_client_matches_session(PipeWireClient *client,
                                            SessionRecord *session)
{
    if (!pipewire_client_is_restricted(client) || session == NULL ||
        session->closed || session->close_requested || session->sources == NULL ||
        session->sources->len == 0) {
        return false;
    }

    for (guint i = 0; i < session->sources->len; i++) {
        ScreenCastSource *source = &g_array_index(session->sources,
                                                  ScreenCastSource, i);
        uint32_t permissions = 0;
        if (!pipewire_client_permission(client, source->node_id, &permissions) ||
            (permissions & PW_PERM_R) == 0) {
            return false;
        }
    }

    BridgeState *state = session->state;
    for (guint i = 0; i < state->sessions->len; i++) {
        SessionRecord *other = g_ptr_array_index(state->sessions, i);
        if (other == session || other->sources == NULL) {
            continue;
        }
        for (guint j = 0; j < other->sources->len; j++) {
            ScreenCastSource *source = &g_array_index(other->sources,
                                                      ScreenCastSource, j);
            uint32_t permissions = 0;
            if (!session_approves_source(session, source->node_id) &&
                pipewire_client_permission(client, source->node_id,
                                           &permissions) &&
                (permissions & PW_PERM_R) != 0) {
                return false;
            }
        }
    }
    return true;
}

static bool source_node_is_approved(SessionRecord *session,
                                    PipeWireNode *node)
{
    return source_generation_matches(session_source_for_id(session, node->id),
                                     node);
}

static PipeWireNode *source_node_for_consumer(PipeWireCompat *compat,
                                              SessionRecord *session,
                                              PipeWireNode *consumer)
{
    if (session->sources->len == 1) {
        ScreenCastSource *source = &g_array_index(session->sources,
                                                  ScreenCastSource, 0);
        PipeWireNode *node = find_pipewire_node(compat, source->node_id);
        return node != NULL && source_node_is_approved(session, node) ? node : NULL;
    }

    if (consumer->target_object == NULL) {
        return NULL;
    }
    uint64_t target = parse_pipewire_serial(consumer->target_object);
    for (guint i = 0; i < session->sources->len; i++) {
        ScreenCastSource *source = &g_array_index(session->sources,
                                                  ScreenCastSource, i);
        if (target != source->node_id &&
            (source->serial == 0 || target != source->serial)) {
            continue;
        }
        PipeWireNode *node = find_pipewire_node(compat, source->node_id);
        if (node != NULL && source_node_is_approved(session, node)) {
            return node;
        }
    }
    return NULL;
}

static bool pipewire_link_exists(PipeWireCompat *compat, SessionRecord *session,
                                 uint32_t source_port_id,
                                 uint32_t consumer_port_id)
{
    for (guint i = 0; i < compat->links->len; i++) {
        PipeWireLink *link = g_ptr_array_index(compat->links, i);
        if (link->proxy != NULL && link->session == session &&
            link->source_port_id == source_port_id &&
            link->consumer_port_id == consumer_port_id) {
            return true;
        }
    }
    return false;
}

static void on_pipewire_link_destroy(void *user_data)
{
    PipeWireLink *link = user_data;
    spa_hook_remove(&link->proxy_listener);
    link->proxy = NULL;
}

static void on_pipewire_link_removed(void *user_data)
{
    PipeWireLink *link = user_data;
    PipeWireCompat *compat = link->compat;
    pw_proxy_destroy(link->proxy);
    g_ptr_array_remove(compat->links, link);
}

static void on_pipewire_link_error(void *user_data, int seq, int result,
                                   const char *message)
{
    (void)seq;
    PipeWireLink *link = user_data;
    log_line("PipeWire compatibility link %u -> %u failed: %s (%s)",
             link->source_node_id, link->consumer_node_id, message,
             spa_strerror(result));
}

static const struct pw_proxy_events PIPEWIRE_LINK_PROXY_EVENTS = {
    PW_VERSION_PROXY_EVENTS,
    .destroy = on_pipewire_link_destroy,
    .removed = on_pipewire_link_removed,
    .error = on_pipewire_link_error,
};

static void create_pipewire_link(PipeWireCompat *compat, SessionRecord *session,
                                 PipeWireClient *client, PipeWireNode *source,
                                 PipeWirePort *source_port,
                                 PipeWireNode *consumer,
                                 PipeWirePort *consumer_port)
{
    if (!session_approves_source(session, source->id) ||
        !source_node_is_approved(session, source) ||
        pipewire_link_exists(compat, session, source_port->id,
                             consumer_port->id)) {
        return;
    }

    char *source_node_id = g_strdup_printf("%u", source->id);
    char *source_port_id = g_strdup_printf("%u", source_port->id);
    char *consumer_node_id = g_strdup_printf("%u", consumer->id);
    char *consumer_port_id = g_strdup_printf("%u", consumer_port->id);
    struct pw_properties *properties = pw_properties_new(
        PW_KEY_LINK_OUTPUT_NODE, source_node_id,
        PW_KEY_LINK_OUTPUT_PORT, source_port_id,
        PW_KEY_LINK_INPUT_NODE, consumer_node_id,
        PW_KEY_LINK_INPUT_PORT, consumer_port_id,
        PW_KEY_OBJECT_LINGER, "false",
        NULL);
    struct pw_proxy *proxy = pw_core_create_object(
        compat->core, "link-factory", PW_TYPE_INTERFACE_Link,
        PW_VERSION_LINK, &properties->dict, 0);
    pw_properties_free(properties);
    g_free(source_node_id);
    g_free(source_port_id);
    g_free(consumer_node_id);
    g_free(consumer_port_id);
    if (proxy == NULL) {
        log_line("create PipeWire compatibility link %u -> %u failed: %s",
                 source->id, consumer->id, g_strerror(errno));
        return;
    }

    PipeWireLink *link = g_new0(PipeWireLink, 1);
    link->compat = compat;
    link->session = session;
    link->proxy = proxy;
    link->source_node_id = source->id;
    link->source_port_id = source_port->id;
    link->consumer_client_id = client->id;
    link->consumer_node_id = consumer->id;
    link->consumer_port_id = consumer_port->id;
    pw_proxy_add_listener(link->proxy, &link->proxy_listener,
                          &PIPEWIRE_LINK_PROXY_EVENTS, link);
    g_ptr_array_add(compat->links, link);
    log_line("linked approved ScreenCast source %u:%u -> portal client %u node %u:%u",
             source->id, source_port->id, client->id, consumer->id,
             consumer_port->id);
}

static void pipewire_compat_try_links(PipeWireCompat *compat)
{
    if (compat == NULL || compat->core == NULL) {
        return;
    }
    for (guint client_index = 0; client_index < compat->clients->len;
         client_index++) {
        PipeWireClient *client = g_ptr_array_index(compat->clients, client_index);
        for (guint session_index = 0;
             session_index < compat->state->sessions->len; session_index++) {
            SessionRecord *session = g_ptr_array_index(compat->state->sessions,
                                                       session_index);
            if (!pipewire_client_matches_session(client, session)) {
                continue;
            }
            for (guint node_index = 0; node_index < compat->nodes->len;
                 node_index++) {
                PipeWireNode *consumer = g_ptr_array_index(compat->nodes,
                                                           node_index);
                if (consumer->client_id != client->id ||
                    g_strcmp0(consumer->media_class, "Stream/Input/Video") != 0) {
                    continue;
                }
                PipeWireNode *source = source_node_for_consumer(
                    compat, session, consumer);
                if (source == NULL) {
                    continue;
                }
                for (guint out_index = 0; out_index < compat->ports->len;
                     out_index++) {
                    PipeWirePort *output = g_ptr_array_index(compat->ports,
                                                             out_index);
                    if (!output->is_output || output->node_id != source->id) {
                        continue;
                    }
                    for (guint in_index = 0; in_index < compat->ports->len;
                         in_index++) {
                        PipeWirePort *input = g_ptr_array_index(compat->ports,
                                                               in_index);
                        if (input->is_input && input->node_id == consumer->id) {
                            create_pipewire_link(compat, session, client, source,
                                                 output, consumer, input);
                        }
                    }
                }
            }
        }
    }
}

static void remove_pipewire_links_for_session(SessionRecord *session)
{
    PipeWireCompat *compat = session != NULL ? session->state->pipewire : NULL;
    if (compat == NULL) {
        return;
    }
    for (guint i = compat->links->len; i > 0; i--) {
        PipeWireLink *link = g_ptr_array_index(compat->links, i - 1);
        if (link->session == session) {
            g_ptr_array_remove_index(compat->links, i - 1);
        }
    }
}

static void remove_pipewire_links_for_object(PipeWireCompat *compat,
                                             uint32_t object_id,
                                             bool client)
{
    for (guint i = compat->links->len; i > 0; i--) {
        PipeWireLink *link = g_ptr_array_index(compat->links, i - 1);
        bool matches = client ? link->consumer_client_id == object_id
                              : link->source_node_id == object_id ||
                                    link->consumer_node_id == object_id ||
                                    link->source_port_id == object_id ||
                                    link->consumer_port_id == object_id;
        if (matches) {
            g_ptr_array_remove_index(compat->links, i - 1);
        }
    }
}

static void on_pipewire_client_permissions(
    void *user_data, uint32_t index, uint32_t n_permissions,
    const struct pw_permission *permissions)
{
    PipeWireClient *client = user_data;
    if (index == 0) {
        g_array_set_size(client->permissions, 0);
    }
    if (index > client->permissions->len) {
        g_array_set_size(client->permissions, index);
    }
    for (uint32_t i = 0; i < n_permissions; i++) {
        if (index + i < client->permissions->len) {
            g_array_index(client->permissions, struct pw_permission,
                          index + i) = permissions[i];
        } else {
            g_array_append_val(client->permissions, permissions[i]);
        }
    }
    client->permissions_received = true;
    pipewire_compat_try_links(client->compat);
}

static const struct pw_client_events PIPEWIRE_CLIENT_EVENTS = {
    PW_VERSION_CLIENT_EVENTS,
    .permissions = on_pipewire_client_permissions,
};

static void refresh_pipewire_client_permissions(PipeWireClient *client)
{
    if (client == NULL || client->proxy == NULL) {
        return;
    }
    int result = pw_client_get_permissions(client->proxy, 0, UINT32_MAX);
    if (result < 0) {
        log_line("read PipeWire portal client %u permissions failed: %s",
                 client->id, spa_strerror(result));
    }
}

static void refresh_pipewire_permissions_for_client(PipeWireCompat *compat,
                                                    uint32_t client_id)
{
    if (compat == NULL) {
        return;
    }
    for (guint i = 0; i < compat->clients->len; i++) {
        PipeWireClient *client = g_ptr_array_index(compat->clients, i);
        if (client_id == SPA_ID_INVALID || client->id == client_id) {
            refresh_pipewire_client_permissions(client);
        }
    }
}

static void on_pipewire_registry_global(
    void *user_data, uint32_t id, uint32_t permissions, const char *type,
    uint32_t version, const struct spa_dict *properties)
{
    (void)permissions;
    PipeWireCompat *compat = user_data;
    if (g_strcmp0(type, PW_TYPE_INTERFACE_Client) == 0) {
        const char *access = spa_dict_lookup(properties, "pipewire.access");
        if (g_strcmp0(access, "portal") != 0) {
            return;
        }
        PipeWireClient *client = g_new0(PipeWireClient, 1);
        client->compat = compat;
        client->id = id;
        client->is_portal = true;
        client->permissions = g_array_new(FALSE, TRUE,
                                          sizeof(struct pw_permission));
        client->proxy = pw_registry_bind(
            compat->registry, id, PW_TYPE_INTERFACE_Client,
            SPA_MIN(version, PW_VERSION_CLIENT), 0);
        if (client->proxy == NULL) {
            free_pipewire_client(client);
            return;
        }
        pw_client_add_listener(client->proxy, &client->listener,
                               &PIPEWIRE_CLIENT_EVENTS, client);
        g_ptr_array_add(compat->clients, client);
        refresh_pipewire_client_permissions(client);
    } else if (g_strcmp0(type, PW_TYPE_INTERFACE_Node) == 0) {
        PipeWireNode *node = g_new0(PipeWireNode, 1);
        node->id = id;
        node->client_id = parse_pipewire_id(
            spa_dict_lookup(properties, PW_KEY_CLIENT_ID));
        node->serial = parse_pipewire_serial(
            spa_dict_lookup(properties, PW_KEY_OBJECT_SERIAL));
        node->media_class = g_strdup(
            spa_dict_lookup(properties, PW_KEY_MEDIA_CLASS));
        node->target_object = g_strdup(
            spa_dict_lookup(properties, PW_KEY_TARGET_OBJECT));
        g_ptr_array_add(compat->nodes, node);
        if (g_strcmp0(node->media_class, "Stream/Input/Video") == 0) {
            refresh_pipewire_permissions_for_client(compat, node->client_id);
        }
        pipewire_compat_try_links(compat);
    } else if (g_strcmp0(type, PW_TYPE_INTERFACE_Port) == 0) {
        PipeWirePort *port = g_new0(PipeWirePort, 1);
        port->id = id;
        port->node_id = parse_pipewire_id(
            spa_dict_lookup(properties, PW_KEY_NODE_ID));
        const char *direction = spa_dict_lookup(properties,
                                                PW_KEY_PORT_DIRECTION);
        port->is_input = g_strcmp0(direction, "in") == 0;
        port->is_output = g_strcmp0(direction, "out") == 0;
        g_ptr_array_add(compat->ports, port);
        pipewire_compat_try_links(compat);
    }
}

static void on_pipewire_registry_global_remove(void *user_data, uint32_t id)
{
    PipeWireCompat *compat = user_data;
    for (guint i = compat->clients->len; i > 0; i--) {
        PipeWireClient *client = g_ptr_array_index(compat->clients, i - 1);
        if (client->id == id) {
            remove_pipewire_links_for_object(compat, id, true);
            g_ptr_array_remove_index(compat->clients, i - 1);
        }
    }
    for (guint i = compat->nodes->len; i > 0; i--) {
        PipeWireNode *node = g_ptr_array_index(compat->nodes, i - 1);
        if (node->id != id) {
            continue;
        }
        remove_pipewire_links_for_object(compat, id, false);
        for (guint session_index = 0;
             session_index < compat->state->sessions->len; session_index++) {
            SessionRecord *session = g_ptr_array_index(compat->state->sessions,
                                                       session_index);
            if (session->sources == NULL) {
                continue;
            }
            remove_session_source_for_node(session, node);
        }
        g_ptr_array_remove_index(compat->nodes, i - 1);
    }
    for (guint i = compat->ports->len; i > 0; i--) {
        PipeWirePort *port = g_ptr_array_index(compat->ports, i - 1);
        if (port->id == id) {
            remove_pipewire_links_for_object(compat, id, false);
            g_ptr_array_remove_index(compat->ports, i - 1);
        }
    }
}

static const struct pw_registry_events PIPEWIRE_REGISTRY_EVENTS = {
    PW_VERSION_REGISTRY_EVENTS,
    .global = on_pipewire_registry_global,
    .global_remove = on_pipewire_registry_global_remove,
};

static void on_pipewire_core_error(void *user_data, uint32_t id, int seq,
                                   int result, const char *message)
{
    (void)user_data;
    (void)seq;
    if (id == PW_ID_CORE) {
        log_line("PipeWire compatibility connection failed: %s (%s)",
                 message, spa_strerror(result));
    }
}

static const struct pw_core_events PIPEWIRE_CORE_EVENTS = {
    PW_VERSION_CORE_EVENTS,
    .error = on_pipewire_core_error,
};

static gboolean pipewire_source_prepare(GSource *source, gint *timeout)
{
    (void)source;
    *timeout = -1;
    return FALSE;
}

static gboolean pipewire_source_dispatch(GSource *source, GSourceFunc callback,
                                          gpointer user_data)
{
    (void)callback;
    (void)user_data;
    PipeWireSource *pipewire_source = (PipeWireSource *)source;
    int result = pw_loop_iterate(
        pw_main_loop_get_loop(pipewire_source->compat->loop), 0);
    if (result < 0) {
        log_line("PipeWire compatibility loop failed: %s",
                 spa_strerror(result));
    }
    return G_SOURCE_CONTINUE;
}

static void pipewire_source_finalize(GSource *source)
{
    PipeWireSource *pipewire_source = (PipeWireSource *)source;
    pw_loop_leave(pw_main_loop_get_loop(pipewire_source->compat->loop));
}

static GSourceFuncs PIPEWIRE_SOURCE_FUNCS = {
    .prepare = pipewire_source_prepare,
    .dispatch = pipewire_source_dispatch,
    .finalize = pipewire_source_finalize,
};

static void free_pipewire_compat(PipeWireCompat *compat)
{
    if (compat == NULL) {
        return;
    }
    if (compat->source != NULL) {
        g_source_destroy(compat->source);
        g_source_unref(compat->source);
    }
    if (compat->links != NULL) {
        g_ptr_array_free(compat->links, TRUE);
    }
    if (compat->clients != NULL) {
        g_ptr_array_free(compat->clients, TRUE);
    }
    if (compat->ports != NULL) {
        g_ptr_array_free(compat->ports, TRUE);
    }
    if (compat->nodes != NULL) {
        g_ptr_array_free(compat->nodes, TRUE);
    }
    if (compat->registry != NULL) {
        spa_hook_remove(&compat->registry_listener);
        pw_proxy_destroy((struct pw_proxy *)compat->registry);
    }
    if (compat->core != NULL) {
        spa_hook_remove(&compat->core_listener);
        pw_core_disconnect(compat->core);
    }
    if (compat->context != NULL) {
        pw_context_destroy(compat->context);
    }
    if (compat->loop != NULL) {
        pw_main_loop_destroy(compat->loop);
    }
    g_free(compat);
}

static PipeWireCompat *new_pipewire_compat(BridgeState *state)
{
    pw_init(NULL, NULL);
    PipeWireCompat *compat = g_new0(PipeWireCompat, 1);
    compat->state = state;
    compat->clients = g_ptr_array_new_with_free_func(
        (GDestroyNotify)free_pipewire_client);
    compat->nodes = g_ptr_array_new_with_free_func(
        (GDestroyNotify)free_pipewire_node);
    compat->ports = g_ptr_array_new_with_free_func(
        (GDestroyNotify)free_pipewire_port);
    compat->links = g_ptr_array_new_with_free_func(
        (GDestroyNotify)free_pipewire_link);
    compat->loop = pw_main_loop_new(NULL);
    if (compat->loop == NULL) {
        free_pipewire_compat(compat);
        return NULL;
    }
    compat->context = pw_context_new(pw_main_loop_get_loop(compat->loop),
                                     NULL, 0);
    if (compat->context == NULL) {
        free_pipewire_compat(compat);
        return NULL;
    }
    struct pw_properties *properties = pw_properties_new(
        PW_KEY_APP_NAME, "freebsd-flatpak portal compatibility", NULL);
    compat->core = pw_context_connect(compat->context, properties, 0);
    if (compat->core == NULL) {
        free_pipewire_compat(compat);
        return NULL;
    }
    pw_core_add_listener(compat->core, &compat->core_listener,
                         &PIPEWIRE_CORE_EVENTS, compat);
    compat->registry = pw_core_get_registry(compat->core,
                                            PW_VERSION_REGISTRY, 0);
    if (compat->registry == NULL) {
        free_pipewire_compat(compat);
        return NULL;
    }
    pw_registry_add_listener(compat->registry, &compat->registry_listener,
                             &PIPEWIRE_REGISTRY_EVENTS, compat);

    PipeWireSource *source = (PipeWireSource *)g_source_new(
        &PIPEWIRE_SOURCE_FUNCS, sizeof(PipeWireSource));
    source->compat = compat;
    struct pw_loop *loop = pw_main_loop_get_loop(compat->loop);
    pw_loop_enter(loop);
    g_source_add_unix_fd(&source->source, pw_loop_get_fd(loop),
                         G_IO_IN | G_IO_ERR | G_IO_HUP);
    compat->source = &source->source;
    g_source_attach(compat->source, NULL);
    log_line("enabled ownership-based PipeWire ScreenCast linking");
    return compat;
}

static GVariant *path_bytes_variant(const char *path)
{
    gsize len = strlen(path) + 1;
    return g_variant_new_fixed_array(G_VARIANT_TYPE_BYTE, path, len, sizeof(guchar));
}

static char **read_permissions(void)
{
    char **permissions = g_new0(char *, 2);
    permissions[0] = g_strdup("read");
    return permissions;
}

static void free_grant(DocumentGrant *grant)
{
    if (grant == NULL) {
        return;
    }
    g_free(grant->doc_id);
    g_free(grant->host_path);
    g_free(grant->placeholder_path);
    if (grant->target_paths != NULL) {
        g_ptr_array_free(grant->target_paths, TRUE);
    }
    g_free(grant->app_id);
    g_strfreev(grant->permissions);
    g_free(grant);
}

static bool run_argv(char **argv, GError **error)
{
    gint status = 0;
    gchar *stderr_text = NULL;
    if (!g_spawn_sync(NULL, argv, NULL, G_SPAWN_SEARCH_PATH, NULL, NULL, NULL, &stderr_text,
                      &status, error)) {
        g_free(stderr_text);
        return false;
    }
    if (!g_spawn_check_wait_status(status, error)) {
        if (stderr_text != NULL && *stderr_text != '\0') {
            log_line("%s", stderr_text);
        }
        g_free(stderr_text);
        return false;
    }
    g_free(stderr_text);
    return true;
}

static bool mount_file_read_only(const char *source, const char *target, GError **error)
{
    char *argv[] = { "doas", "mount_nullfs", "-o", "ro", (char *)source, (char *)target, NULL };
    return run_argv(argv, error);
}

static bool unmount_path(const char *target)
{
    GError *error = NULL;
    char *argv[] = { "doas", "umount", (char *)target, NULL };
    if (run_argv(argv, &error)) {
        return true;
    }
    if (error != NULL) {
        log_line("umount failed for %s: %s", target, error->message);
        g_error_free(error);
    }

    error = NULL;
    char *force_argv[] = { "doas", "umount", "-f", (char *)target, NULL };
    if (run_argv(force_argv, &error)) {
        return true;
    }
    if (error != NULL) {
        log_line("forced umount failed for %s: %s", target, error->message);
        g_error_free(error);
    }
    return false;
}

static void cleanup_grant(DocumentGrant *grant)
{
    if (grant == NULL) {
        return;
    }
    if (grant->target_paths != NULL) {
        for (guint i = 0; i < grant->target_paths->len; i++) {
            unmount_path(g_ptr_array_index(grant->target_paths, i));
        }
        g_ptr_array_set_size(grant->target_paths, 0);
    }
    const char *placeholder = grant->placeholder_path;
    if (placeholder == NULL) {
        return;
    }
    if (g_remove(placeholder) != 0 && errno != ENOENT) {
        log_line("remove %s failed: %s", placeholder, g_strerror(errno));
    }
    char *dir = g_path_get_dirname(placeholder);
    if (g_rmdir(dir) != 0 && errno != ENOENT) {
        log_line("remove %s failed: %s", dir, g_strerror(errno));
    }
    g_free(dir);
}

static bool sandbox_doc_dir_allowed(BridgeState *state, const char *path)
{
    if (path == NULL || !g_path_is_absolute(path)) {
        return false;
    }
    char *root_prefix = g_strconcat(state->sandbox_root, G_DIR_SEPARATOR_S, NULL);
    bool allowed = g_str_has_prefix(path, root_prefix) && strstr(path, "/../") == NULL &&
                   !g_str_has_suffix(path, "/..");
    g_free(root_prefix);
    return allowed;
}

static bool mount_grant_in_sandbox(DocumentGrant *grant, const char *sandbox_doc_dir,
                                   GError **error)
{
    char *base = g_path_get_basename(grant->host_path);
    char *target_dir = g_build_filename(sandbox_doc_dir, grant->doc_id, NULL);
    char *target = g_build_filename(target_dir, base, NULL);
    g_free(base);
    g_free(target_dir);

    if (!mount_file_read_only(grant->host_path, target, error)) {
        g_free(target);
        return false;
    }
    g_ptr_array_add(grant->target_paths, target);
    return true;
}

static void remove_sandbox_grants(BridgeState *state, const char *sandbox_doc_dir)
{
    char *prefix = g_strconcat(sandbox_doc_dir, G_DIR_SEPARATOR_S, NULL);
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        for (guint j = grant->target_paths->len; j > 0; j--) {
            const char *target = g_ptr_array_index(grant->target_paths, j - 1);
            if (g_str_has_prefix(target, prefix)) {
                unmount_path(target);
                g_ptr_array_remove_index(grant->target_paths, j - 1);
            }
        }
    }
    g_free(prefix);
}

static void free_request(RequestRecord *request)
{
    if (request == NULL) {
        return;
    }
    if (request->host_signal_id != 0 && request->state->host_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus, request->host_signal_id);
    }
    if (request->local_registration_id != 0 && request->state->local_bus != NULL) {
        g_dbus_connection_unregister_object(request->state->local_bus,
                                            request->local_registration_id);
    }
    g_free(request->client_sender);
    g_free(request->local_path);
    g_free(request->host_path);
    g_free(request->local_session_path);
    g_free(request);
}

static void free_session(SessionRecord *session)
{
    if (session == NULL) {
        return;
    }
    remove_pipewire_links_for_session(session);
    if (session->host_signal_id != 0 && session->state->host_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(session->state->host_bus,
                                             session->host_signal_id);
    }
    if (session->local_registration_id != 0 && session->state->local_bus != NULL) {
        g_dbus_connection_unregister_object(session->state->local_bus,
                                            session->local_registration_id);
    }
    g_free(session->client_sender);
    g_free(session->local_path);
    g_free(session->host_path);
    if (session->sources != NULL) {
        g_array_free(session->sources, TRUE);
    }
    g_free(session);
}

static void free_menu_proxy(MenuProxy *menu)
{
    if (menu == NULL) {
        return;
    }
    if (menu->local_signal_id != 0 && menu->item->state->local_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(menu->item->state->local_bus,
                                             menu->local_signal_id);
    }
    if (menu->host_registration_id != 0 && menu->item->state->host_bus != NULL) {
        g_dbus_connection_unregister_object(menu->item->state->host_bus,
                                            menu->host_registration_id);
    }
    g_free(menu->local_path);
    g_free(menu->host_path);
    g_free(menu);
}

static void free_status_item(StatusItem *item)
{
    if (item == NULL) {
        return;
    }
    if (item->local_signal_id != 0 && item->state->local_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(item->state->local_bus, item->local_signal_id);
    }
    if (item->host_registration_id != 0 && item->state->host_bus != NULL) {
        g_dbus_connection_unregister_object(item->state->host_bus,
                                            item->host_registration_id);
    }
    if (item->menus != NULL) {
        g_ptr_array_free(item->menus, TRUE);
    }
    g_free(item->local_service);
    g_free(item->local_path);
    g_free(item->local_registration);
    g_free(item->host_path);
    g_free(item);
}

static void cleanup_all(BridgeState *state)
{
    for (guint i = 0; i < state->grants->len; i++) {
        cleanup_grant(g_ptr_array_index(state->grants, i));
    }
    g_ptr_array_set_size(state->grants, 0);
}

static gboolean handle_signal(gpointer user_data)
{
    BridgeState *state = user_data;
    cleanup_all(state);
    if (state->loop != NULL) {
        g_main_loop_quit(state->loop);
    }
    return G_SOURCE_REMOVE;
}

static bool fd_host_path(int fd, char *path, size_t path_len, GError **error)
{
    struct kinfo_file info;
    memset(&info, 0, sizeof(info));
    info.kf_structsize = sizeof(info);
    if (fcntl(fd, F_KINFO, &info) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "F_KINFO failed: %s",
                    g_strerror(errno));
        return false;
    }
    if (info.kf_type != KF_TYPE_VNODE || info.kf_un.kf_file.kf_file_type != KF_VTYPE_VREG ||
        info.kf_path[0] == '\0') {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                    "selected FD is not a regular path-backed file");
        return false;
    }
    g_strlcpy(path, info.kf_path, path_len);
    return true;
}

static char *safe_doc_id(BridgeState *state)
{
    GString *id = g_string_new("freebsd_flatpak_poc_");
    for (const char *p = state->app_id; *p != '\0'; p++) {
        if (g_ascii_isalnum(*p)) {
            g_string_append_c(id, *p);
        } else {
            g_string_append_c(id, '_');
        }
    }
    g_string_append_printf(id, "_%" G_GUINT64_FORMAT, ++state->counter);
    return g_string_free(id, FALSE);
}

static char *safe_path_element(const char *input)
{
    GString *value = g_string_new("");
    for (const char *p = input; p != NULL && *p != '\0'; p++) {
        if (g_ascii_isalnum(*p) || *p == '_') {
            g_string_append_c(value, *p);
        } else {
            g_string_append_c(value, '_');
        }
    }
    if (value->len == 0) {
        g_string_append(value, "x");
    }
    return g_string_free(value, FALSE);
}

static char *sender_path_element(const char *sender)
{
    if (sender != NULL && sender[0] == ':') {
        sender++;
    }
    return safe_path_element(sender);
}

static char *portal_path(const char *kind, const char *sender, const char *token)
{
    char *sender_element = sender_path_element(sender);
    char *token_element = safe_path_element(token);
    char *path = g_strdup_printf("/org/freedesktop/portal/desktop/%s/%s/%s",
                                 kind, sender_element, token_element);
    g_free(sender_element);
    g_free(token_element);
    return path;
}

static char **permissions_from_variant(GVariant *permissions)
{
    GPtrArray *items = g_ptr_array_new_with_free_func(g_free);
    GVariantIter iter;
    const char *permission = NULL;
    g_variant_iter_init(&iter, permissions);
    while (g_variant_iter_next(&iter, "&s", &permission)) {
        g_ptr_array_add(items, g_strdup(permission));
    }
    if (items->len == 0) {
        g_ptr_array_add(items, g_strdup("read"));
    }
    g_ptr_array_add(items, NULL);
    return (char **)g_ptr_array_free(items, FALSE);
}

static bool create_document_grant_from_path(BridgeState *state, const char *host_path,
                                            const char *app_id, char **permissions,
                                            DocumentGrant **out, GError **error)
{
    struct stat st;
    if (g_stat(host_path, &st) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "stat %s failed: %s",
                    host_path, g_strerror(errno));
        return false;
    }
    if (!S_ISREG(st.st_mode)) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_REGULAR_FILE,
                    "FileChooser V1 only grants regular files: %s", host_path);
        return false;
    }

    char *base = g_path_get_basename(host_path);
    char *doc_id = safe_doc_id(state);
    char *source_doc_dir = g_build_filename(state->doc_dir, doc_id, NULL);
    if (g_mkdir_with_parents(source_doc_dir, 0700) != 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "create %s failed: %s",
                    source_doc_dir, g_strerror(errno));
        g_free(base);
        g_free(doc_id);
        g_free(source_doc_dir);
        return false;
    }

    char *placeholder = g_build_filename(source_doc_dir, base, NULL);
    int placeholder_fd = g_open(placeholder, O_CREAT | O_TRUNC | O_WRONLY, 0600);
    if (placeholder_fd < 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno), "create %s failed: %s",
                    placeholder, g_strerror(errno));
        g_free(base);
        g_free(doc_id);
        g_free(source_doc_dir);
        g_free(placeholder);
        return false;
    }
    close(placeholder_fd);

    DocumentGrant *grant = g_new0(DocumentGrant, 1);
    grant->doc_id = doc_id;
    grant->host_path = g_strdup(host_path);
    grant->placeholder_path = placeholder;
    grant->target_paths = g_ptr_array_new_with_free_func(g_free);
    grant->app_id = g_strdup(app_id != NULL && *app_id != '\0' ? app_id : state->app_id);
    grant->permissions = permissions != NULL ? g_strdupv(permissions) : read_permissions();

    for (guint i = 0; i < state->sandbox_doc_dirs->len; i++) {
        if (!mount_grant_in_sandbox(
                grant, g_ptr_array_index(state->sandbox_doc_dirs, i), error)) {
            cleanup_grant(grant);
            free_grant(grant);
            g_free(base);
            g_free(source_doc_dir);
            return false;
        }
    }
    *out = grant;

    log_line("%s -> %u sandbox(s) as %s/%s", grant->host_path,
             grant->target_paths->len, grant->doc_id, base);
    g_free(base);
    g_free(source_doc_dir);
    return true;
}

static bool create_document_grant_from_fd(BridgeState *state, int fd, const char *app_id,
                                          GVariant *permissions, DocumentGrant **out,
                                          GError **error)
{
    char host_path[PATH_MAX];
    if (!fd_host_path(fd, host_path, sizeof(host_path), error)) {
        return false;
    }
    char **permission_list = permissions_from_variant(permissions);
    bool ok = create_document_grant_from_path(state, host_path, app_id, permission_list, out,
                                              error);
    g_strfreev(permission_list);
    return ok;
}

static DocumentGrant *find_grant(BridgeState *state, const char *doc_id)
{
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        if (g_strcmp0(grant->doc_id, doc_id) == 0) {
            return grant;
        }
    }
    return NULL;
}

static void add_mountpoint_extra(BridgeState *state, GVariantBuilder *extra)
{
    g_variant_builder_add(extra, "{sv}", "mountpoint", path_bytes_variant(state->mountpoint));
}

static char *sandbox_uri_for_grant(BridgeState *state, DocumentGrant *grant)
{
    char *base = g_path_get_basename(grant->host_path);
    char *sandbox_path = g_build_filename(state->mountpoint, grant->doc_id, base, NULL);
    GError *error = NULL;
    char *uri = g_filename_to_uri(sandbox_path, NULL, &error);
    if (uri == NULL) {
        log_line("could not encode %s as URI: %s", sandbox_path, error->message);
        g_error_free(error);
    }
    g_free(base);
    g_free(sandbox_path);
    return uri;
}

static char *rewrite_file_uri(BridgeState *state, const char *uri)
{
    GError *error = NULL;
    char *host_path = g_filename_from_uri(uri, NULL, &error);
    if (host_path == NULL) {
        log_line("could not decode FileChooser URI %s: %s", uri, error->message);
        g_error_free(error);
        return NULL;
    }

    char **permissions = read_permissions();
    DocumentGrant *grant = NULL;
    if (!create_document_grant_from_path(state, host_path, state->app_id, permissions, &grant,
                                         &error)) {
        log_line("could not grant %s: %s", host_path, error->message);
        g_error_free(error);
        g_strfreev(permissions);
        g_free(host_path);
        return NULL;
    }
    g_strfreev(permissions);
    g_free(host_path);
    g_ptr_array_add(state->grants, grant);

    char *rewritten = sandbox_uri_for_grant(state, grant);
    if (rewritten != NULL) {
        log_line("rewrote FileChooser URI to %s", rewritten);
    }
    return rewritten;
}

static GVariant *rewrite_uri_array(BridgeState *state, GVariant *uris)
{
    GVariantBuilder rewritten;
    g_variant_builder_init(&rewritten, G_VARIANT_TYPE("as"));

    GVariantIter iter;
    const char *uri = NULL;
    g_variant_iter_init(&iter, uris);
    while (g_variant_iter_next(&iter, "&s", &uri)) {
        char *mapped = NULL;
        if (g_str_has_prefix(uri, "file://")) {
            mapped = rewrite_file_uri(state, uri);
        }
        g_variant_builder_add(&rewritten, "s", mapped != NULL ? mapped : uri);
        g_free(mapped);
    }

    return g_variant_builder_end(&rewritten);
}

static GVariant *rewrite_filechooser_results(BridgeState *state, guint32 response,
                                             GVariant *results)
{
    GVariantBuilder out;
    g_variant_builder_init(&out, G_VARIANT_TYPE("a{sv}"));

    GVariantIter iter;
    const char *key = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, results);
    while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
        if (response == 0 && g_strcmp0(key, "uris") == 0 &&
            g_variant_is_of_type(value, G_VARIANT_TYPE("as"))) {
            GVariant *rewritten = rewrite_uri_array(state, value);
            g_variant_builder_add(&out, "{sv}", key, rewritten);
        } else {
            g_variant_builder_add(&out, "{sv}", key, value);
        }
        g_variant_unref(value);
    }

    return g_variant_builder_end(&out);
}

static RequestRecord *find_request(BridgeState *state, const char *local_path)
{
    for (guint i = 0; i < state->requests->len; i++) {
        RequestRecord *request = g_ptr_array_index(state->requests, i);
        if (g_strcmp0(request->local_path, local_path) == 0) {
            return request;
        }
    }
    return NULL;
}

static void emit_request_response(RequestRecord *request, guint32 response, GVariant *results)
{
    if (request->completed) {
        g_variant_unref(results);
        return;
    }
    request->completed = true;
    GError *error = NULL;
    if (!g_dbus_connection_emit_signal(request->state->local_bus, request->client_sender,
                                       request->local_path,
                                       "org.freedesktop.portal.Request", "Response",
                                       g_variant_new("(u@a{sv})", response, results), &error)) {
        log_line("emit Response to %s failed: %s", request->client_sender, error->message);
        g_error_free(error);
        return;
    }
    log_line("emitted Response %u to %s on %s", response, request->client_sender,
             request->local_path);
}

static void emit_cancel_response(RequestRecord *request)
{
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
    emit_request_response(request, 2, g_variant_builder_end(&results));
}

static void on_host_response(GDBusConnection *connection, const gchar *sender_name,
                             const gchar *object_path, const gchar *interface_name,
                             const gchar *signal_name, GVariant *parameters,
                             gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    (void)interface_name;
    (void)signal_name;

    RequestRecord *request = user_data;
    guint32 response = 2;
    GVariant *results = NULL;
    g_variant_get(parameters, "(u@a{sv})", &response, &results);
    GVariant *rewritten = rewrite_filechooser_results(request->state, response, results);
    g_variant_unref(results);
    emit_request_response(request, response, rewritten);

    if (request->host_signal_id != 0) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus, request->host_signal_id);
        request->host_signal_id = 0;
    }
}

static void on_host_filechooser_call(GObject *source_object, GAsyncResult *result,
                                     gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    RequestRecord *request = user_data;
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
    if (reply == NULL) {
        log_line("host FileChooser call failed: %s", error->message);
        g_error_free(error);
        emit_cancel_response(request);
        return;
    }

    const char *host_handle = NULL;
    g_variant_get(reply, "(&o)", &host_handle);
    g_free(request->host_path);
    request->host_path = g_strdup(host_handle);
    request->host_signal_id = g_dbus_connection_signal_subscribe(
        request->state->host_bus, "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Request", "Response", host_handle, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, on_host_response, request, NULL);
    if (request->close_requested) {
        g_dbus_connection_call(request->state->host_bus,
                               "org.freedesktop.portal.Desktop", host_handle,
                               "org.freedesktop.portal.Request", "Close",
                               NULL, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
                               NULL, NULL, NULL);
    }
    g_variant_unref(reply);
}

static GVariant *option_value(GVariant *options, const char *key)
{
    GVariantIter iter;
    const char *name = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, options);
    while (g_variant_iter_next(&iter, "{&sv}", &name, &value)) {
        if (g_strcmp0(name, key) == 0) {
            return value;
        }
        g_variant_unref(value);
    }
    return NULL;
}

static char *token_from_options(BridgeState *state, GVariant *options, const char *key,
                                const char *fallback)
{
    GVariant *token_value = option_value(options, key);
    char *token = NULL;
    if (token_value != NULL && g_variant_is_of_type(token_value, G_VARIANT_TYPE_STRING)) {
        token = safe_path_element(g_variant_get_string(token_value, NULL));
        g_variant_unref(token_value);
    } else {
        if (token_value != NULL) {
            g_variant_unref(token_value);
        }
        token = g_strdup_printf("%s_%" G_GUINT64_FORMAT, fallback,
                                ++state->request_counter);
    }
    return token;
}

static char *request_path_for_options(BridgeState *state, const char *sender, GVariant *options)
{
    char *token = token_from_options(state, options, "handle_token",
                                     "freebsd_flatpak_poc");
    char *path = portal_path("request", sender, token);
    g_free(token);
    return path;
}

static char *request_path_for_call(BridgeState *state, const char *sender,
                                   GVariant *parameters, gsize options_index)
{
    GVariant *options = g_variant_get_child_value(parameters, options_index);
    char *path = request_path_for_options(state, sender, options);
    g_variant_unref(options);
    return path;
}

static char *fresh_host_token(BridgeState *state, const char *label)
{
    return g_strdup_printf("freebsd_flatpak_%s_%" G_GUINT64_FORMAT, label,
                           ++state->host_token_counter);
}

static GVariant *rewrite_options(GVariant *options, const char *handle_token,
                                 const char *session_token)
{
    GVariantBuilder out;
    g_variant_builder_init(&out, G_VARIANT_TYPE_VARDICT);
    GVariantIter iter;
    const char *key = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, options);
    while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
        if (g_strcmp0(key, "handle_token") != 0 &&
            (session_token == NULL || g_strcmp0(key, "session_handle_token") != 0)) {
            g_variant_builder_add(&out, "{sv}", key, value);
        }
        g_variant_unref(value);
    }
    if (handle_token != NULL) {
        g_variant_builder_add(&out, "{sv}", "handle_token",
                              g_variant_new_string(handle_token));
    }
    if (session_token != NULL) {
        g_variant_builder_add(&out, "{sv}", "session_handle_token",
                              g_variant_new_string(session_token));
    }
    return g_variant_builder_end(&out);
}

static void handle_request_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)interface_name;
    (void)parameters;

    BridgeState *state = user_data;
    RequestRecord *request = find_request(state, object_path);
    if (request == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                              "unknown request object");
        return;
    }
    if (g_strcmp0(sender, request->client_sender) != 0) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_PERMISSION_DENIED,
                                              "request belongs to another client");
        return;
    }
    if (g_strcmp0(method_name, "Close") == 0) {
        request->close_requested = true;
        request->completed = true;
        if (request->host_path != NULL) {
            g_dbus_connection_call(request->state->host_bus,
                                   "org.freedesktop.portal.Desktop",
                                   request->host_path,
                                   "org.freedesktop.portal.Request", "Close",
                                   NULL, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
                                   NULL, NULL, NULL);
        }
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                          "%s is not implemented", method_name);
}

static GVariant *handle_request_property(GDBusConnection *connection, const gchar *sender,
                                         const gchar *object_path,
                                         const gchar *interface_name,
                                         const gchar *property_name, GError **error,
                                         gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)user_data;
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s", property_name);
    return NULL;
}

static const GDBusInterfaceVTable REQUEST_VTABLE = {
    .method_call = handle_request_method,
    .get_property = handle_request_property,
};

static SessionRecord *find_session(BridgeState *state, const char *local_path)
{
    for (guint i = 0; i < state->sessions->len; i++) {
        SessionRecord *session = g_ptr_array_index(state->sessions, i);
        if (g_strcmp0(session->local_path, local_path) == 0) {
            return session;
        }
    }
    return NULL;
}

static void close_host_session(SessionRecord *session)
{
    remove_pipewire_links_for_session(session);
    if (session->closed || session->close_requested || session->host_path == NULL ||
        session->state->host_bus == NULL) {
        return;
    }
    session->close_requested = true;
    g_dbus_connection_call(session->state->host_bus,
                           "org.freedesktop.portal.Desktop", session->host_path,
                           "org.freedesktop.portal.Session", "Close", NULL, NULL,
                           G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
}

static void on_host_session_closed(GDBusConnection *connection, const gchar *sender_name,
                                   const gchar *object_path, const gchar *interface_name,
                                   const gchar *signal_name, GVariant *parameters,
                                   gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    (void)interface_name;
    (void)signal_name;
    SessionRecord *session = user_data;
    if (session->closed) {
        return;
    }
    remove_pipewire_links_for_session(session);
    session->closed = true;
    GError *error = NULL;
    if (!g_dbus_connection_emit_signal(session->state->local_bus,
                                       session->client_sender, session->local_path,
                                       "org.freedesktop.portal.Session", "Closed",
                                       g_variant_ref(parameters), &error)) {
        log_line("emit Session.Closed to %s failed: %s",
                 session->client_sender, error->message);
        g_error_free(error);
    }
}

static void handle_session_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)interface_name;
    (void)parameters;
    BridgeState *state = user_data;
    SessionRecord *session = find_session(state, object_path);
    if (session == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_NOT_FOUND,
                                              "unknown session object");
        return;
    }
    if (g_strcmp0(sender, session->client_sender) != 0) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_PERMISSION_DENIED,
                                              "session belongs to another client");
        return;
    }
    if (g_strcmp0(method_name, "Close") == 0) {
        close_host_session(session);
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_NOT_SUPPORTED,
                                          "%s is not implemented", method_name);
}

static GVariant *handle_session_property(GDBusConnection *connection, const gchar *sender,
                                         const gchar *object_path,
                                         const gchar *interface_name,
                                         const gchar *property_name, GError **error,
                                         gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)user_data;
    if (g_strcmp0(property_name, "version") == 0) {
        return g_variant_new_uint32(1);
    }
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                "unknown property %s", property_name);
    return NULL;
}

static const GDBusInterfaceVTable SESSION_VTABLE = {
    .method_call = handle_session_method,
    .get_property = handle_session_property,
};

static SessionRecord *register_session(RequestRecord *request, const char *host_path,
                                       GError **error)
{
    BridgeState *state = request->state;
    if (find_session(state, request->local_session_path) != NULL) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_EXISTS,
                    "session already exists: %s", request->local_session_path);
        return NULL;
    }
    SessionRecord *session = g_new0(SessionRecord, 1);
    session->state = state;
    session->client_sender = g_strdup(request->client_sender);
    session->local_path = g_strdup(request->local_session_path);
    session->host_path = g_strdup(host_path);
    session->sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource));
    GDBusInterfaceInfo *iface =
        g_dbus_node_info_lookup_interface(state->session_node,
                                          "org.freedesktop.portal.Session");
    session->local_registration_id = g_dbus_connection_register_object(
        state->local_bus, session->local_path, iface, &SESSION_VTABLE,
        state, NULL, error);
    if (session->local_registration_id == 0) {
        free_session(session);
        return NULL;
    }
    session->host_signal_id = g_dbus_connection_signal_subscribe(
        state->host_bus, "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Session", "Closed", session->host_path, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, on_host_session_closed, session, NULL);
    g_ptr_array_add(state->sessions, session);
    log_line("mapped ScreenCast session %s -> %s",
             session->local_path, session->host_path);
    return session;
}

static GVariant *rewrite_create_session_results(RequestRecord *request, guint32 response,
                                                GVariant *results, guint32 *out_response)
{
    *out_response = response;
    if (response != 0) {
        return g_variant_ref(results);
    }
    const char *host_session = NULL;
    if (!g_variant_lookup(results, "session_handle", "&s", &host_session)) {
        log_line("host CreateSession response omitted session_handle");
        *out_response = 2;
        GVariantBuilder empty;
        g_variant_builder_init(&empty, G_VARIANT_TYPE_VARDICT);
        return g_variant_builder_end(&empty);
    }
    GError *error = NULL;
    if (register_session(request, host_session, &error) == NULL) {
        log_line("register local ScreenCast session failed: %s", error->message);
        g_error_free(error);
        *out_response = 2;
        GVariantBuilder empty;
        g_variant_builder_init(&empty, G_VARIANT_TYPE_VARDICT);
        return g_variant_builder_end(&empty);
    }

    GVariantBuilder out;
    g_variant_builder_init(&out, G_VARIANT_TYPE_VARDICT);
    GVariantIter iter;
    const char *key = NULL;
    GVariant *value = NULL;
    g_variant_iter_init(&iter, results);
    while (g_variant_iter_next(&iter, "{&sv}", &key, &value)) {
        if (g_strcmp0(key, "session_handle") == 0) {
            g_variant_builder_add(&out, "{sv}", key,
                                  g_variant_new_string(request->local_session_path));
        } else {
            g_variant_builder_add(&out, "{sv}", key, value);
        }
        g_variant_unref(value);
    }
    return g_variant_builder_end(&out);
}

static void update_session_sources(SessionRecord *session, GVariant *results)
{
    GVariant *streams = g_variant_lookup_value(
        results, "streams", G_VARIANT_TYPE("a(ua{sv})"));
    if (streams == NULL) {
        log_line("ScreenCast.Start response omitted streams");
        return;
    }

    remove_pipewire_links_for_session(session);
    g_array_set_size(session->sources, 0);
    GVariantIter iter;
    guint32 node_id = SPA_ID_INVALID;
    GVariant *properties = NULL;
    g_variant_iter_init(&iter, streams);
    while (g_variant_iter_next(&iter, "(u@a{sv})", &node_id, &properties)) {
        ScreenCastSource source = {
            .node_id = node_id,
            .serial = 0,
        };
        g_variant_lookup(properties, "pipewire-serial", "t", &source.serial);
        if (!session_approves_source(session, node_id)) {
            g_array_append_val(session->sources, source);
            log_line("approved ScreenCast source node %u (serial %" G_GUINT64_FORMAT
                     ") for session %s", node_id, source.serial,
                     session->local_path);
        }
        g_variant_unref(properties);
    }
    g_variant_unref(streams);
    refresh_pipewire_permissions_for_client(session->state->pipewire,
                                            SPA_ID_INVALID);
    pipewire_compat_try_links(session->state->pipewire);
}

static void on_host_screencast_response(GDBusConnection *connection,
                                        const gchar *sender_name,
                                        const gchar *object_path,
                                        const gchar *interface_name,
                                        const gchar *signal_name,
                                        GVariant *parameters, gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    (void)interface_name;
    (void)signal_name;
    RequestRecord *request = user_data;
    guint32 response = 2;
    GVariant *results = NULL;
    g_variant_get(parameters, "(u@a{sv})", &response, &results);
    if (request->close_requested) {
        g_variant_unref(results);
        if (request->host_signal_id != 0) {
            g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                                 request->host_signal_id);
            request->host_signal_id = 0;
        }
        return;
    }
    if (response == 0 && request->kind == REQUEST_SCREENCAST_START &&
        request->session != NULL) {
        update_session_sources(request->session, results);
    }
    GVariant *forwarded = request->kind == REQUEST_SCREENCAST_CREATE
                              ? rewrite_create_session_results(request, response,
                                                               results, &response)
                              : g_variant_ref(results);
    g_variant_unref(results);
    emit_request_response(request, response, forwarded);
    if (request->host_signal_id != 0) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                             request->host_signal_id);
        request->host_signal_id = 0;
    }
}

static void subscribe_host_request(RequestRecord *request)
{
    if (request->host_signal_id != 0) {
        g_dbus_connection_signal_unsubscribe(request->state->host_bus,
                                             request->host_signal_id);
    }
    request->host_signal_id = g_dbus_connection_signal_subscribe(
        request->state->host_bus, "org.freedesktop.portal.Desktop",
        "org.freedesktop.portal.Request", "Response", request->host_path, NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, on_host_screencast_response, request, NULL);
}

static void on_host_screencast_call(GObject *source_object, GAsyncResult *result,
                                    gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    RequestRecord *request = user_data;
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
    if (reply == NULL) {
        log_line("host ScreenCast call failed: %s", error->message);
        g_error_free(error);
        emit_cancel_response(request);
        return;
    }
    const char *actual_path = NULL;
    g_variant_get(reply, "(&o)", &actual_path);
    if (g_strcmp0(actual_path, request->host_path) != 0) {
        log_line("host returned unexpected request path %s (predicted %s)",
                 actual_path, request->host_path);
        g_free(request->host_path);
        request->host_path = g_strdup(actual_path);
        subscribe_host_request(request);
    }
    if (request->close_requested) {
        g_dbus_connection_call(request->state->host_bus,
                               "org.freedesktop.portal.Desktop",
                               request->host_path,
                               "org.freedesktop.portal.Request", "Close",
                               NULL, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
                               NULL, NULL, NULL);
    }
    g_variant_unref(reply);
}

static RequestRecord *register_screencast_request(BridgeState *state,
                                                  const char *sender,
                                                  GVariant *options,
                                                  RequestKind kind,
                                                  const char *host_token,
                                                  GError **error)
{
    RequestRecord *request = g_new0(RequestRecord, 1);
    request->state = state;
    request->client_sender = g_strdup(sender);
    request->local_path = request_path_for_options(state, sender, options);
    request->kind = kind;
    const char *host_sender = g_dbus_connection_get_unique_name(state->host_bus);
    request->host_path = portal_path("request", host_sender, host_token);
    GDBusInterfaceInfo *iface =
        g_dbus_node_info_lookup_interface(state->request_node,
                                          "org.freedesktop.portal.Request");
    request->local_registration_id = g_dbus_connection_register_object(
        state->local_bus, request->local_path, iface, &REQUEST_VTABLE,
        state, NULL, error);
    if (request->local_registration_id == 0) {
        free_request(request);
        return NULL;
    }
    subscribe_host_request(request);
    g_ptr_array_add(state->requests, request);
    return request;
}

static SessionRecord *owned_session(BridgeState *state, const char *sender,
                                    const char *local_path,
                                    GDBusMethodInvocation *invocation)
{
    SessionRecord *session = find_session(state, local_path);
    if (session == NULL || session->closed || session->close_requested) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_NOT_FOUND,
                                              "unknown or closed session: %s",
                                              local_path);
        return NULL;
    }
    if (g_strcmp0(sender, session->client_sender) != 0) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_PERMISSION_DENIED,
                                              "session belongs to another client");
        return NULL;
    }
    return session;
}

static void handle_screencast_create(BridgeState *state, const char *sender,
                                     GVariant *parameters,
                                     GDBusMethodInvocation *invocation)
{
    GVariant *options = g_variant_get_child_value(parameters, 0);
    char *host_handle_token = fresh_host_token(state, "request");
    char *host_session_token = fresh_host_token(state, "session");
    GError *error = NULL;
    RequestRecord *request = register_screencast_request(
        state, sender, options, REQUEST_SCREENCAST_CREATE,
        host_handle_token, &error);
    if (request == NULL) {
        g_dbus_method_invocation_take_error(invocation, error);
        goto out;
    }
    char *local_session_token = token_from_options(
        state, options, "session_handle_token", "freebsd_flatpak_session");
    request->local_session_path = portal_path("session", sender,
                                              local_session_token);
    g_free(local_session_token);
    GVariant *host_options = rewrite_options(options, host_handle_token,
                                             host_session_token);
    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop",
                           "org.freedesktop.portal.ScreenCast", "CreateSession",
                           g_variant_new("(@a{sv})", host_options),
                           G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, -1,
                           NULL, on_host_screencast_call, request);
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(o)", request->local_path));
    log_line("forwarded ScreenCast.CreateSession as %s", request->local_path);
out:
    g_free(host_handle_token);
    g_free(host_session_token);
    g_variant_unref(options);
}

static void handle_screencast_request(BridgeState *state, const char *sender,
                                      const char *method_name,
                                      GVariant *parameters,
                                      GDBusMethodInvocation *invocation)
{
    const char *local_session_path = NULL;
    g_variant_get_child(parameters, 0, "&o", &local_session_path);
    SessionRecord *session = owned_session(state, sender, local_session_path,
                                           invocation);
    if (session == NULL) {
        return;
    }
    gsize options_index = g_strcmp0(method_name, "Start") == 0 ? 2 : 1;
    GVariant *options = g_variant_get_child_value(parameters, options_index);
    char *host_token = fresh_host_token(state, "request");
    GError *error = NULL;
    bool is_start = g_strcmp0(method_name, "Start") == 0;
    RequestRecord *request = register_screencast_request(
        state, sender, options,
        is_start ? REQUEST_SCREENCAST_START : REQUEST_SCREENCAST_OTHER,
        host_token, &error);
    if (request == NULL) {
        g_dbus_method_invocation_take_error(invocation, error);
        g_free(host_token);
        g_variant_unref(options);
        return;
    }
    request->session = session;
    GVariant *host_options = rewrite_options(options, host_token, NULL);
    GVariant *host_parameters = NULL;
    if (is_start) {
        const char *parent_window = NULL;
        g_variant_get_child(parameters, 1, "&s", &parent_window);
        host_parameters = g_variant_new("(os@a{sv})", session->host_path,
                                        parent_window, host_options);
    } else {
        host_parameters = g_variant_new("(o@a{sv})", session->host_path,
                                        host_options);
    }
    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop",
                           "org.freedesktop.portal.ScreenCast", method_name,
                           host_parameters, G_VARIANT_TYPE("(o)"),
                           G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           on_host_screencast_call, request);
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(o)", request->local_path));
    log_line("forwarded ScreenCast.%s as %s", method_name,
             request->local_path);
    g_free(host_token);
    g_variant_unref(options);
}

static gint32 copy_unix_fd(GUnixFDList *source_fds, gint32 source_index,
                           GUnixFDList *destination_fds, GError **error)
{
    if (source_fds == NULL) {
        g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_DATA,
                    "host portal returned no Unix FD list");
        return -1;
    }
    int source_fd = g_unix_fd_list_get(source_fds, source_index, error);
    if (source_fd < 0) {
        return -1;
    }
    gint32 destination_index = g_unix_fd_list_append(destination_fds, source_fd,
                                                     error);
    close(source_fd);
    return destination_index;
}

static void on_open_pipewire_remote(GObject *source_object, GAsyncResult *result,
                                    gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    GDBusMethodInvocation *invocation = user_data;
    GError *error = NULL;
    GUnixFDList *host_fds = NULL;
    GVariant *reply = g_dbus_connection_call_with_unix_fd_list_finish(
        connection, &host_fds, result, &error);
    if (reply == NULL) {
        g_dbus_method_invocation_take_error(invocation, error);
        g_object_unref(invocation);
        return;
    }
    gint32 host_index = -1;
    g_variant_get(reply, "(h)", &host_index);
    GUnixFDList *local_fds = g_unix_fd_list_new();
    gint32 local_index = copy_unix_fd(host_fds, host_index, local_fds, &error);
    if (local_index < 0) {
        g_dbus_method_invocation_take_error(invocation, error);
    } else {
        g_dbus_method_invocation_return_value_with_unix_fd_list(
            invocation, g_variant_new("(h)", local_index), local_fds);
        log_line("forwarded restricted PipeWire remote fd");
    }
    g_object_unref(local_fds);
    g_variant_unref(reply);
    if (host_fds != NULL) {
        g_object_unref(host_fds);
    }
    g_object_unref(invocation);
}

static void handle_open_pipewire_remote(BridgeState *state, const char *sender,
                                        GVariant *parameters,
                                        GDBusMethodInvocation *invocation)
{
    const char *local_session_path = NULL;
    GVariant *options = NULL;
    g_variant_get(parameters, "(&o@a{sv})", &local_session_path, &options);
    SessionRecord *session = owned_session(state, sender, local_session_path,
                                           invocation);
    if (session == NULL) {
        g_variant_unref(options);
        return;
    }
    g_dbus_connection_call_with_unix_fd_list(
        state->host_bus, "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop",
        "org.freedesktop.portal.ScreenCast", "OpenPipeWireRemote",
        g_variant_new("(o@a{sv})", session->host_path, options),
        G_VARIANT_TYPE("(h)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL,
        on_open_pipewire_remote, g_object_ref(invocation));
}

static void handle_filechooser_open(BridgeState *state, const char *sender,
                                    GVariant *parameters,
                                    GDBusMethodInvocation *invocation)
{
    GError *error = NULL;
    char *local_path = request_path_for_call(state, sender, parameters, 2);
    GDBusInterfaceInfo *request_iface =
        g_dbus_node_info_lookup_interface(state->request_node,
                                          "org.freedesktop.portal.Request");
    RequestRecord *request = g_new0(RequestRecord, 1);
    request->state = state;
    request->client_sender = g_strdup(sender);
    request->local_path = g_strdup(local_path);
    request->kind = REQUEST_FILECHOOSER;
    request->local_registration_id = g_dbus_connection_register_object(
        state->local_bus, local_path, request_iface, &REQUEST_VTABLE, state, NULL, &error);
    if (request->local_registration_id == 0) {
        g_dbus_method_invocation_take_error(invocation, error);
        g_free(request->client_sender);
        g_free(request->local_path);
        g_free(request);
        g_free(local_path);
        return;
    }
    g_ptr_array_add(state->requests, request);

    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop",
                           "org.freedesktop.portal.FileChooser", "OpenFile", parameters,
                           G_VARIANT_TYPE("(o)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           on_host_filechooser_call, request);

    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", local_path));
    log_line("forwarded FileChooser.OpenFile as %s", local_path);
    g_free(local_path);
}

static void on_forward_call(GObject *source_object, GAsyncResult *result, gpointer user_data)
{
    GDBusConnection *connection = G_DBUS_CONNECTION(source_object);
    GDBusMethodInvocation *invocation = user_data;
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_finish(connection, result, &error);
    if (reply == NULL) {
        g_dbus_method_invocation_take_error(invocation, error);
    } else {
        g_dbus_method_invocation_return_value(invocation, reply);
    }
    g_object_unref(invocation);
}

static GVariant *empty_icon_pixmap(void)
{
    GVariantBuilder builder;
    g_variant_builder_init(&builder, G_VARIANT_TYPE("a(iiay)"));
    return g_variant_builder_end(&builder);
}

static GVariant *empty_tooltip(void)
{
    GVariantBuilder pixmap;
    g_variant_builder_init(&pixmap, G_VARIANT_TYPE("a(iiay)"));
    return g_variant_new("(s@a(iiay)ss)", "", g_variant_builder_end(&pixmap), "", "");
}

static GVariant *default_status_property(StatusItem *item, const char *property_name)
{
    if (g_strcmp0(property_name, "Category") == 0) {
        return g_variant_new_string("ApplicationStatus");
    }
    if (g_strcmp0(property_name, "Id") == 0 || g_strcmp0(property_name, "Title") == 0) {
        return g_variant_new_string(item->state->app_id);
    }
    if (g_strcmp0(property_name, "Status") == 0) {
        return g_variant_new_string("Active");
    }
    if (g_strcmp0(property_name, "WindowId") == 0) {
        return g_variant_new_uint32(0);
    }
    if (g_str_has_suffix(property_name, "IconName") ||
        g_strcmp0(property_name, "AttentionMovieName") == 0) {
        return g_variant_new_string("");
    }
    if (g_str_has_suffix(property_name, "IconPixmap")) {
        return empty_icon_pixmap();
    }
    if (g_strcmp0(property_name, "ToolTip") == 0) {
        return empty_tooltip();
    }
    if (g_strcmp0(property_name, "ItemIsMenu") == 0) {
        return g_variant_new_boolean(FALSE);
    }
    if (g_strcmp0(property_name, "Menu") == 0) {
        return g_variant_new_object_path("/");
    }
    return NULL;
}

static void on_local_status_signal(GDBusConnection *connection, const gchar *sender_name,
                                   const gchar *object_path, const gchar *interface_name,
                                   const gchar *signal_name, GVariant *parameters,
                                   gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    StatusItem *item = user_data;
    if (!g_dbus_connection_emit_signal(item->state->host_bus, NULL, item->host_path,
                                       interface_name, signal_name, g_variant_ref(parameters),
                                       NULL)) {
        log_line("forward StatusNotifier signal %s failed", signal_name);
    }
}

static void on_local_menu_signal(GDBusConnection *connection, const gchar *sender_name,
                                 const gchar *object_path, const gchar *interface_name,
                                 const gchar *signal_name, GVariant *parameters,
                                 gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    MenuProxy *menu = user_data;
    if (!g_dbus_connection_emit_signal(menu->item->state->host_bus, NULL, menu->host_path,
                                       interface_name, signal_name, g_variant_ref(parameters),
                                       NULL)) {
        log_line("forward DBusMenu signal %s failed", signal_name);
    }
}

static void handle_menu_method(GDBusConnection *connection, const gchar *sender,
                               const gchar *object_path, const gchar *interface_name,
                               const gchar *method_name, GVariant *parameters,
                               GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    MenuProxy *menu = user_data;
    g_dbus_connection_call(menu->item->state->local_bus, menu->item->local_service,
                           menu->local_path, interface_name, method_name, parameters, NULL,
                           G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_forward_call,
                           g_object_ref(invocation));
}

static const GDBusInterfaceVTable MENU_VTABLE = {
    .method_call = handle_menu_method,
};

static MenuProxy *ensure_menu_proxy(StatusItem *item, const char *menu_path)
{
    if (menu_path == NULL || g_strcmp0(menu_path, "/") == 0 || *menu_path == '\0') {
        return NULL;
    }
    for (guint i = 0; i < item->menus->len; i++) {
        MenuProxy *menu = g_ptr_array_index(item->menus, i);
        if (g_strcmp0(menu->local_path, menu_path) == 0) {
            return menu;
        }
    }

    GDBusInterfaceInfo *iface =
        g_dbus_node_info_lookup_interface(item->state->dbusmenu_node,
                                          "com.canonical.dbusmenu");
    MenuProxy *menu = g_new0(MenuProxy, 1);
    menu->item = item;
    menu->local_path = g_strdup(menu_path);
    menu->host_path =
        g_strdup_printf("%s/Menu%u", item->host_path, item->menus->len + 1);

    GError *error = NULL;
    menu->host_registration_id =
        g_dbus_connection_register_object(item->state->host_bus, menu->host_path, iface,
                                          &MENU_VTABLE, menu, NULL, &error);
    if (menu->host_registration_id == 0) {
        log_line("register host DBusMenu proxy %s failed: %s", menu->host_path,
                 error->message);
        g_error_free(error);
        free_menu_proxy(menu);
        return NULL;
    }
    menu->local_signal_id = g_dbus_connection_signal_subscribe(
        item->state->local_bus, item->local_service, "com.canonical.dbusmenu", NULL,
        menu->local_path, NULL, G_DBUS_SIGNAL_FLAGS_NONE, on_local_menu_signal, menu, NULL);

    g_ptr_array_add(item->menus, menu);
    log_line("bridged DBusMenu %s -> host %s", menu->local_path, menu->host_path);
    return menu;
}

static GVariant *local_status_property(StatusItem *item, const char *property_name)
{
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_sync(
        item->state->local_bus, item->local_service, item->local_path,
        "org.freedesktop.DBus.Properties", "Get",
        g_variant_new("(ss)", "org.kde.StatusNotifierItem", property_name),
        G_VARIANT_TYPE("(v)"), G_DBUS_CALL_FLAGS_NONE, 1000, NULL, &error);
    if (reply == NULL) {
        log_line("StatusNotifier property %s.%s unavailable: %s", item->local_service,
                 property_name, error->message);
        g_error_free(error);
        return default_status_property(item, property_name);
    }

    GVariant *boxed = g_variant_get_child_value(reply, 0);
    GVariant *value = g_variant_get_variant(boxed);
    g_variant_unref(boxed);
    g_variant_unref(reply);

    if (g_strcmp0(property_name, "Menu") == 0 &&
        g_variant_is_of_type(value, G_VARIANT_TYPE_OBJECT_PATH)) {
        MenuProxy *menu = ensure_menu_proxy(item, g_variant_get_string(value, NULL));
        if (menu != NULL) {
            g_variant_unref(value);
            return g_variant_new_object_path(menu->host_path);
        }
    }
    return value;
}

static void handle_status_item_method(GDBusConnection *connection, const gchar *sender,
                                      const gchar *object_path, const gchar *interface_name,
                                      const gchar *method_name, GVariant *parameters,
                                      GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    StatusItem *item = user_data;
    g_dbus_connection_call(item->state->local_bus, item->local_service, item->local_path,
                           interface_name, method_name, parameters, NULL,
                           G_DBUS_CALL_FLAGS_NONE, -1, NULL, on_forward_call,
                           g_object_ref(invocation));
}

static GVariant *handle_status_item_property(GDBusConnection *connection, const gchar *sender,
                                             const gchar *object_path,
                                             const gchar *interface_name,
                                             const gchar *property_name, GError **error,
                                             gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)error;
    StatusItem *item = user_data;
    return local_status_property(item, property_name);
}

static const GDBusInterfaceVTable STATUS_ITEM_VTABLE = {
    .method_call = handle_status_item_method,
    .get_property = handle_status_item_property,
};

static char *fresh_request_path(BridgeState *state, const char *label)
{
    return g_strdup_printf("/org/freedesktop/portal/desktop/request/freebsd_flatpak_poc/%s_%" G_GUINT64_FORMAT,
                           label, ++state->request_counter);
}

static void return_immediate_empty_request(BridgeState *state, GDBusMethodInvocation *invocation,
                                           const char *label)
{
    char *path = fresh_request_path(state, label);
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(o)", path));
    GVariantBuilder results;
    g_variant_builder_init(&results, G_VARIANT_TYPE("a{sv}"));
    g_dbus_connection_emit_signal(state->local_bus, NULL, path,
                                  "org.freedesktop.portal.Request", "Response",
                                  g_variant_new("(u@a{sv})", 2,
                                                g_variant_builder_end(&results)),
                                  NULL);
    g_free(path);
}

static void forward_desktop_method(BridgeState *state, const char *interface_name,
                                   const char *method_name, GVariant *parameters,
                                   GDBusMethodInvocation *invocation)
{
    g_dbus_connection_call(state->host_bus, "org.freedesktop.portal.Desktop",
                           "/org/freedesktop/portal/desktop", interface_name, method_name,
                           parameters, NULL, G_DBUS_CALL_FLAGS_NONE, -1, NULL,
                           on_forward_call, g_object_ref(invocation));
}

static void return_settings_readall(GDBusMethodInvocation *invocation)
{
    GVariantBuilder values;
    g_variant_builder_init(&values, G_VARIANT_TYPE("a{sa{sv}}"));
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{sa{sv}})", &values));
}

static void return_proxy_direct(GDBusMethodInvocation *invocation)
{
    const char *direct[] = { "direct://", NULL };
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(^as)", direct));
}

static void handle_desktop_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)object_path;

    BridgeState *state = user_data;
    if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0 &&
        g_strcmp0(method_name, "CreateSession") == 0) {
        handle_screencast_create(state, sender, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0 &&
               (g_strcmp0(method_name, "SelectSources") == 0 ||
                g_strcmp0(method_name, "Start") == 0)) {
        handle_screencast_request(state, sender, method_name, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0 &&
               g_strcmp0(method_name, "OpenPipeWireRemote") == 0) {
        handle_open_pipewire_remote(state, sender, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0 &&
        g_strcmp0(method_name, "OpenFile") == 0) {
        handle_filechooser_open(state, sender, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s is not implemented by this V1 bridge",
                                              method_name);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0 &&
               g_strcmp0(method_name, "ReadAll") == 0) {
        forward_desktop_method(state, interface_name, method_name, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0 &&
               g_strcmp0(method_name, "Read") == 0) {
        forward_desktop_method(state, interface_name, method_name, parameters, invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.ProxyResolver") == 0 &&
               g_strcmp0(method_name, "Lookup") == 0) {
        return_proxy_direct(invocation);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
               (g_strcmp0(method_name, "CreateMonitor") == 0 ||
                g_strcmp0(method_name, "Inhibit") == 0)) {
        return_immediate_empty_request(state, invocation, method_name);
    } else if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0 &&
               g_strcmp0(method_name, "QueryEndResponse") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s.%s is not implemented by this V1 bridge",
                                              interface_name, method_name);
    }
}

static void handle_add_full(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *parameters = g_dbus_method_invocation_get_parameters(invocation);
    GVariant *handles = g_variant_get_child_value(parameters, 0);
    GVariant *permissions = g_variant_get_child_value(parameters, 3);
    const char *app_id = NULL;
    g_variant_get_child(parameters, 2, "&s", &app_id);

    GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
    GUnixFDList *fd_list = g_dbus_message_get_unix_fd_list(message);
    if (fd_list == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                                              "AddFull did not include an fd list");
        g_variant_unref(handles);
        g_variant_unref(permissions);
        return;
    }

    GVariantBuilder ids;
    g_variant_builder_init(&ids, G_VARIANT_TYPE("as"));
    for (gsize i = 0; i < g_variant_n_children(handles); i++) {
        gint32 handle = -1;
        g_variant_get_child(handles, i, "h", &handle);

        GError *error = NULL;
        int fd = g_unix_fd_list_get(fd_list, handle, &error);
        if (fd < 0) {
            g_dbus_method_invocation_take_error(invocation, error);
            g_variant_unref(handles);
            g_variant_unref(permissions);
            return;
        }

        DocumentGrant *grant = NULL;
        if (!create_document_grant_from_fd(state, fd, app_id, permissions, &grant, &error)) {
            close(fd);
            g_dbus_method_invocation_take_error(invocation, error);
            g_variant_unref(handles);
            g_variant_unref(permissions);
            return;
        }
        close(fd);
        g_ptr_array_add(state->grants, grant);
        g_variant_builder_add(&ids, "s", grant->doc_id);
    }

    GVariantBuilder extra;
    g_variant_builder_init(&extra, G_VARIANT_TYPE("a{sv}"));
    add_mountpoint_extra(state, &extra);
    g_dbus_method_invocation_return_value(invocation,
                                          g_variant_new("(as@a{sv})", &ids,
                                                        g_variant_builder_end(&extra)));
    g_variant_unref(handles);
    g_variant_unref(permissions);
}

static void handle_delete(BridgeState *state, GDBusMethodInvocation *invocation)
{
    const char *doc_id = NULL;
    g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)", &doc_id);
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        if (g_strcmp0(grant->doc_id, doc_id) == 0) {
            cleanup_grant(grant);
            g_ptr_array_remove_index(state->grants, i);
            g_dbus_method_invocation_return_value(invocation, NULL);
            return;
        }
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                          "No such document: %s", doc_id);
}

static void return_lookup(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *path_variant =
        g_variant_get_child_value(g_dbus_method_invocation_get_parameters(invocation), 0);
    gsize size = 0;
    const gchar *path = g_variant_get_fixed_array(path_variant, &size, sizeof(guchar));
    const char *doc_id = "";
    if (path != NULL) {
        for (guint i = 0; i < state->grants->len; i++) {
            DocumentGrant *grant = g_ptr_array_index(state->grants, i);
            if (g_strcmp0(grant->host_path, path) == 0) {
                doc_id = grant->doc_id;
                break;
            }
        }
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(s)", doc_id));
    g_variant_unref(path_variant);
}

static void return_info(BridgeState *state, GDBusMethodInvocation *invocation)
{
    const char *doc_id = NULL;
    g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)", &doc_id);
    DocumentGrant *grant = find_grant(state, doc_id);
    if (grant == NULL) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_FOUND,
                                              "No such document: %s", doc_id);
        return;
    }

    GVariantBuilder apps;
    g_variant_builder_init(&apps, G_VARIANT_TYPE("a{sas}"));
    GVariantBuilder permissions;
    g_variant_builder_init(&permissions, G_VARIANT_TYPE("as"));
    for (char **p = grant->permissions; p != NULL && *p != NULL; p++) {
        g_variant_builder_add(&permissions, "s", *p);
    }
    g_variant_builder_add(&apps, "{s@as}", grant->app_id, g_variant_builder_end(&permissions));
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@aya{sas})", path_bytes_variant(grant->host_path), &apps));
}

static void return_list(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariantBuilder docs;
    g_variant_builder_init(&docs, G_VARIANT_TYPE("a{say}"));
    for (guint i = 0; i < state->grants->len; i++) {
        DocumentGrant *grant = g_ptr_array_index(state->grants, i);
        g_variant_builder_add(&docs, "{s@ay}", grant->doc_id, path_bytes_variant(grant->host_path));
    }
    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{say})", &docs));
}

static void return_host_paths(BridgeState *state, GDBusMethodInvocation *invocation)
{
    GVariant *doc_ids =
        g_variant_get_child_value(g_dbus_method_invocation_get_parameters(invocation), 0);
    GVariantBuilder paths;
    g_variant_builder_init(&paths, G_VARIANT_TYPE("a{say}"));

    GVariantIter iter;
    const char *doc_id = NULL;
    g_variant_iter_init(&iter, doc_ids);
    while (g_variant_iter_next(&iter, "&s", &doc_id)) {
        DocumentGrant *grant = find_grant(state, doc_id);
        if (grant != NULL) {
            g_variant_builder_add(&paths, "{s@ay}", grant->doc_id,
                                  path_bytes_variant(grant->host_path));
        }
    }

    g_dbus_method_invocation_return_value(invocation, g_variant_new("(a{say})", &paths));
    g_variant_unref(doc_ids);
}

static void handle_documents_method(GDBusConnection *connection, const gchar *sender,
                                    const gchar *object_path, const gchar *interface_name,
                                    const gchar *method_name, GVariant *parameters,
                                    GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    (void)parameters;

    BridgeState *state = user_data;
    if (g_strcmp0(method_name, "GetMountPoint") == 0) {
        g_dbus_method_invocation_return_value(invocation,
                                              g_variant_new("(@ay)",
                                                            path_bytes_variant(state->mountpoint)));
    } else if (g_strcmp0(method_name, "AddFull") == 0) {
        handle_add_full(state, invocation);
    } else if (g_strcmp0(method_name, "GrantPermissions") == 0 ||
               g_strcmp0(method_name, "RevokePermissions") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
    } else if (g_strcmp0(method_name, "Delete") == 0) {
        handle_delete(state, invocation);
    } else if (g_strcmp0(method_name, "Lookup") == 0) {
        return_lookup(state, invocation);
    } else if (g_strcmp0(method_name, "Info") == 0) {
        return_info(state, invocation);
    } else if (g_strcmp0(method_name, "List") == 0) {
        return_list(state, invocation);
    } else if (g_strcmp0(method_name, "GetHostPaths") == 0) {
        return_host_paths(state, invocation);
    } else {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                              "%s is not implemented by this V1 bridge",
                                              method_name);
    }
}

static GVariant *handle_get_property(GDBusConnection *connection, const gchar *sender,
                                     const gchar *object_path, const gchar *interface_name,
                                     const gchar *property_name, GError **error,
                                     gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    BridgeState *state = user_data;

    if (g_strcmp0(interface_name, "org.freedesktop.portal.ScreenCast") == 0) {
        if (g_strcmp0(property_name, "version") == 0) {
            return g_variant_new_uint32(state->screencast_version);
        }
        if (g_strcmp0(property_name, "AvailableSourceTypes") == 0) {
            return g_variant_new_uint32(state->screencast_source_types);
        }
        if (g_strcmp0(property_name, "AvailableCursorModes") == 0) {
            return g_variant_new_uint32(state->screencast_cursor_modes);
        }
    }

    if (g_strcmp0(property_name, "version") == 0) {
        if (g_strcmp0(interface_name, "org.freedesktop.portal.FileChooser") == 0) {
            return g_variant_new_uint32(4);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Documents") == 0) {
            return g_variant_new_uint32(5);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Settings") == 0) {
            return g_variant_new_uint32(2);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.ProxyResolver") == 0) {
            return g_variant_new_uint32(1);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.Inhibit") == 0) {
            return g_variant_new_uint32(3);
        }
        if (g_strcmp0(interface_name, "org.freedesktop.portal.FileTransfer") == 0) {
            return g_variant_new_uint32(1);
        }
    }
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s.%s",
                interface_name, property_name);
    return NULL;
}

static const GDBusInterfaceVTable DESKTOP_VTABLE = {
    .method_call = handle_desktop_method,
    .get_property = handle_get_property,
};

static const GDBusInterfaceVTable DOCUMENTS_VTABLE = {
    .method_call = handle_documents_method,
    .get_property = handle_get_property,
};

static char *status_registration_string(const char *service)
{
    return g_strdup(service);
}

static StatusItem *find_status_item(BridgeState *state, const char *local_service,
                                    const char *local_path)
{
    for (guint i = 0; i < state->status_items->len; i++) {
        StatusItem *item = g_ptr_array_index(state->status_items, i);
        if (g_strcmp0(item->local_service, local_service) == 0 &&
            g_strcmp0(item->local_path, local_path) == 0) {
            return item;
        }
    }
    return NULL;
}

static bool register_host_status_item(StatusItem *item, GError **error)
{
    GDBusInterfaceInfo *iface =
        g_dbus_node_info_lookup_interface(item->state->status_item_node,
                                          "org.kde.StatusNotifierItem");
    item->host_registration_id =
        g_dbus_connection_register_object(item->state->host_bus, item->host_path, iface,
                                          &STATUS_ITEM_VTABLE, item, NULL, error);
    if (item->host_registration_id == 0) {
        return false;
    }

    item->local_signal_id = g_dbus_connection_signal_subscribe(
        item->state->local_bus, item->local_service, "org.kde.StatusNotifierItem", NULL,
        item->local_path, NULL, G_DBUS_SIGNAL_FLAGS_NONE, on_local_status_signal, item, NULL);
    return true;
}

static bool register_with_host_watcher(StatusItem *item, GError **error)
{
    GVariant *reply = g_dbus_connection_call_sync(
        item->state->host_bus, "org.kde.StatusNotifierWatcher", "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher", "RegisterStatusNotifierItem",
        g_variant_new("(s)", item->host_path), NULL, G_DBUS_CALL_FLAGS_NONE, 2000, NULL,
        error);
    if (reply == NULL) {
        return false;
    }
    g_variant_unref(reply);
    return true;
}

static void emit_local_status_item_registered(StatusItem *item)
{
    g_dbus_connection_emit_signal(
        item->state->local_bus, NULL, "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher", "StatusNotifierItemRegistered",
        g_variant_new("(s)", item->local_registration), NULL);
}

static void handle_register_status_item(BridgeState *state, const char *sender,
                                        GVariant *parameters,
                                        GDBusMethodInvocation *invocation)
{
    const char *service = NULL;
    g_variant_get(parameters, "(&s)", &service);
    const char *local_service = service;
    const char *local_path = "/StatusNotifierItem";
    if (g_str_has_prefix(service, "/")) {
        local_service = sender;
        local_path = service;
    }

    StatusItem *existing = find_status_item(state, local_service, local_path);
    if (existing != NULL) {
        emit_local_status_item_registered(existing);
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }

    StatusItem *item = g_new0(StatusItem, 1);
    item->state = state;
    item->local_service = g_strdup(local_service);
    item->local_path = g_strdup(local_path);
    item->local_registration = status_registration_string(service);
    item->host_path =
        g_strdup_printf("/StatusNotifierItem/freebsd_flatpak_poc_%" G_GUINT64_FORMAT,
                        ++state->status_counter);
    item->menus = g_ptr_array_new_with_free_func((GDestroyNotify)free_menu_proxy);

    GError *error = NULL;
    if (!register_host_status_item(item, &error)) {
        g_dbus_method_invocation_take_error(invocation, error);
        free_status_item(item);
        return;
    }
    if (!register_with_host_watcher(item, &error)) {
        g_dbus_method_invocation_take_error(invocation, error);
        free_status_item(item);
        return;
    }

    g_ptr_array_add(state->status_items, item);
    emit_local_status_item_registered(item);
    g_dbus_method_invocation_return_value(invocation, NULL);
    log_line("bridged StatusNotifierItem %s%s -> host %s", item->local_service,
             item->local_path, item->host_path);
}

static void handle_status_watcher_method(GDBusConnection *connection, const gchar *sender,
                                         const gchar *object_path,
                                         const gchar *interface_name,
                                         const gchar *method_name, GVariant *parameters,
                                         GDBusMethodInvocation *invocation,
                                         gpointer user_data)
{
    (void)connection;
    (void)object_path;
    (void)interface_name;
    BridgeState *state = user_data;
    if (g_strcmp0(method_name, "RegisterStatusNotifierItem") == 0) {
        handle_register_status_item(state, sender, parameters, invocation);
        return;
    }
    if (g_strcmp0(method_name, "RegisterStatusNotifierHost") == 0) {
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
                                          "%s is not implemented", method_name);
}

static GVariant *handle_status_watcher_property(GDBusConnection *connection,
                                                const gchar *sender,
                                                const gchar *object_path,
                                                const gchar *interface_name,
                                                const gchar *property_name, GError **error,
                                                gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    BridgeState *state = user_data;
    if (g_strcmp0(property_name, "RegisteredStatusNotifierItems") == 0) {
        GVariantBuilder items;
        g_variant_builder_init(&items, G_VARIANT_TYPE("as"));
        for (guint i = 0; i < state->status_items->len; i++) {
            StatusItem *item = g_ptr_array_index(state->status_items, i);
            g_variant_builder_add(&items, "s", item->local_registration);
        }
        return g_variant_builder_end(&items);
    }
    if (g_strcmp0(property_name, "IsStatusNotifierHostRegistered") == 0) {
        return g_variant_new_boolean(TRUE);
    }
    if (g_strcmp0(property_name, "ProtocolVersion") == 0) {
        return g_variant_new_int32(0);
    }
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_NOT_FOUND, "unknown property %s",
                property_name);
    return NULL;
}

static const GDBusInterfaceVTable STATUS_WATCHER_VTABLE = {
    .method_call = handle_status_watcher_method,
    .get_property = handle_status_watcher_property,
};

static gint find_sandbox_doc_dir(BridgeState *state, const char *path)
{
    for (guint i = 0; i < state->sandbox_doc_dirs->len; i++) {
        if (g_strcmp0(g_ptr_array_index(state->sandbox_doc_dirs, i), path) == 0) {
            return (gint)i;
        }
    }
    return -1;
}

static void handle_control_method(GDBusConnection *connection, const gchar *sender,
                                  const gchar *object_path, const gchar *interface_name,
                                  const gchar *method_name, GVariant *parameters,
                                  GDBusMethodInvocation *invocation, gpointer user_data)
{
    (void)connection;
    (void)sender;
    (void)object_path;
    (void)interface_name;
    BridgeState *state = user_data;
    const char *sandbox_doc_dir = NULL;
    g_variant_get(parameters, "(&s)", &sandbox_doc_dir);
    if (!sandbox_doc_dir_allowed(state, sandbox_doc_dir)) {
        g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                              G_IO_ERROR_PERMISSION_DENIED,
                                              "sandbox document directory is outside %s",
                                              state->sandbox_root);
        return;
    }

    gint index = find_sandbox_doc_dir(state, sandbox_doc_dir);
    if (g_strcmp0(method_name, "AddSandbox") == 0) {
        if (index >= 0) {
            g_dbus_method_invocation_return_value(invocation, NULL);
            return;
        }
        for (guint i = 0; i < state->grants->len; i++) {
            GError *error = NULL;
            if (!mount_grant_in_sandbox(g_ptr_array_index(state->grants, i),
                                        sandbox_doc_dir, &error)) {
                remove_sandbox_grants(state, sandbox_doc_dir);
                g_dbus_method_invocation_take_error(invocation, error);
                return;
            }
        }
        g_ptr_array_add(state->sandbox_doc_dirs, g_strdup(sandbox_doc_dir));
        log_line("attached sandbox document root %s", sandbox_doc_dir);
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    if (g_strcmp0(method_name, "RemoveSandbox") == 0) {
        if (index >= 0) {
            remove_sandbox_grants(state, sandbox_doc_dir);
            g_ptr_array_remove_index(state->sandbox_doc_dirs, (guint)index);
            log_line("detached sandbox document root %s", sandbox_doc_dir);
        }
        g_dbus_method_invocation_return_value(invocation, NULL);
        return;
    }
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_NOT_SUPPORTED,
                                          "%s is not implemented", method_name);
}

static const GDBusInterfaceVTable CONTROL_VTABLE = {
    .method_call = handle_control_method,
};

static void load_host_screencast_properties(BridgeState *state)
{
    GError *error = NULL;
    GVariant *reply = g_dbus_connection_call_sync(
        state->host_bus, "org.freedesktop.portal.Desktop",
        "/org/freedesktop/portal/desktop", "org.freedesktop.DBus.Properties",
        "GetAll", g_variant_new("(s)", "org.freedesktop.portal.ScreenCast"),
        G_VARIANT_TYPE("(a{sv})"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
    if (reply == NULL) {
        log_line("read host ScreenCast properties failed: %s", error->message);
        g_error_free(error);
        return;
    }

    GVariant *properties = NULL;
    g_variant_get(reply, "(@a{sv})", &properties);
    g_variant_lookup(properties, "version", "u", &state->screencast_version);
    g_variant_lookup(properties, "AvailableSourceTypes", "u",
                     &state->screencast_source_types);
    g_variant_lookup(properties, "AvailableCursorModes", "u",
                     &state->screencast_cursor_modes);
    log_line("host ScreenCast version=%u source-types=%u cursor-modes=%u",
             state->screencast_version, state->screencast_source_types,
             state->screencast_cursor_modes);
    g_variant_unref(properties);
    g_variant_unref(reply);
}

static void close_resources_for_client(BridgeState *state, const char *client_sender)
{
    for (guint i = 0; i < state->requests->len; i++) {
        RequestRecord *request = g_ptr_array_index(state->requests, i);
        if (request->completed ||
            g_strcmp0(request->client_sender, client_sender) != 0) {
            continue;
        }
        request->close_requested = true;
        request->completed = true;
        if (request->host_path != NULL) {
            g_dbus_connection_call(state->host_bus,
                                   "org.freedesktop.portal.Desktop",
                                   request->host_path,
                                   "org.freedesktop.portal.Request", "Close",
                                   NULL, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
                                   NULL, NULL, NULL);
        }
    }
    for (guint i = 0; i < state->sessions->len; i++) {
        SessionRecord *session = g_ptr_array_index(state->sessions, i);
        if (g_strcmp0(session->client_sender, client_sender) == 0) {
            close_host_session(session);
        }
    }
}

static void on_local_name_owner_changed(GDBusConnection *connection,
                                        const gchar *sender_name,
                                        const gchar *object_path,
                                        const gchar *interface_name,
                                        const gchar *signal_name,
                                        GVariant *parameters,
                                        gpointer user_data)
{
    (void)connection;
    (void)sender_name;
    (void)object_path;
    (void)interface_name;
    (void)signal_name;
    const char *name = NULL;
    const char *old_owner = NULL;
    const char *new_owner = NULL;
    g_variant_get(parameters, "(&s&s&s)", &name, &old_owner, &new_owner);
    if (name[0] == ':' && old_owner[0] != '\0' && new_owner[0] == '\0') {
        close_resources_for_client(user_data, name);
    }
}

static bool register_node_interfaces(GDBusConnection *connection, const char *path,
                                     GDBusNodeInfo *node, const GDBusInterfaceVTable *vtable,
                                     BridgeState *state, GError **error)
{
    for (guint i = 0; node->interfaces[i] != NULL; i++) {
        guint id =
            g_dbus_connection_register_object(connection, path, node->interfaces[i], vtable,
                                              state, NULL, error);
        if (id == 0) {
            return false;
        }
    }
    return true;
}

static void on_bus_acquired(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)name;
    BridgeState *state = user_data;
    if (state->local_bus == NULL) {
        state->local_bus = g_object_ref(connection);
    }
    if (state->local_objects_registered) {
        return;
    }

    state->local_name_signal_id = g_dbus_connection_signal_subscribe(
        connection, "org.freedesktop.DBus", "org.freedesktop.DBus",
        "NameOwnerChanged", "/org/freedesktop/DBus", NULL,
        G_DBUS_SIGNAL_FLAGS_NONE, on_local_name_owner_changed, state, NULL);

    GError *error = NULL;
    if (!register_node_interfaces(connection, "/org/freedesktop/portal/desktop",
                                  state->desktop_node, &DESKTOP_VTABLE, state, &error)) {
        log_line("register desktop portal failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    if (!register_node_interfaces(connection, "/org/freedesktop/portal/documents",
                                  state->documents_node, &DOCUMENTS_VTABLE, state, &error)) {
        log_line("register documents portal failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    if (!register_node_interfaces(connection, "/StatusNotifierWatcher",
                                  state->status_watcher_node, &STATUS_WATCHER_VTABLE,
                                  state, &error)) {
        log_line("register StatusNotifierWatcher failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    if (!register_node_interfaces(connection, "/org/freebsd/Flatpak/PortalBridge",
                                  state->control_node, &CONTROL_VTABLE, state, &error)) {
        log_line("register sandbox control failed: %s", error->message);
        g_error_free(error);
        g_main_loop_quit(state->loop);
        return;
    }
    state->local_objects_registered = true;
}

static void on_name_acquired(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)connection;
    (void)user_data;
    log_line("acquired %s", name);
}

static void on_name_lost(GDBusConnection *connection, const gchar *name, gpointer user_data)
{
    (void)connection;
    BridgeState *state = user_data;
    log_line("lost %s", name);
    g_main_loop_quit(state->loop);
}

static const char *arg_value(int argc, char **argv, const char *name)
{
    for (int i = 1; i + 1 < argc; i++) {
        if (strcmp(argv[i], name) == 0) {
            return argv[i + 1];
        }
    }
    return NULL;
}

static GDBusConnection *connect_to_bus_address(const char *address, GError **error)
{
    return g_dbus_connection_new_for_address_sync(
        address,
        G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
            G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
        NULL, NULL, error);
}

int main(int argc, char **argv)
{
    const char *app_id = arg_value(argc, argv, "--app-id");
    const char *doc_dir = arg_value(argc, argv, "--doc-dir");
    const char *sandbox_root = arg_value(argc, argv, "--sandbox-root");
    const char *mountpoint = arg_value(argc, argv, "--mountpoint");
    const char *host_bus_address = getenv("HOST_DBUS_SESSION_BUS_ADDRESS");
    if (app_id == NULL || doc_dir == NULL || sandbox_root == NULL || mountpoint == NULL ||
        host_bus_address == NULL || *host_bus_address == '\0') {
        fprintf(stderr,
                "usage: %s --app-id APP_ID --doc-dir HOST_DOC_DIR --sandbox-root APP_CHROOT_ROOT --mountpoint SANDBOX_MOUNTPOINT\n",
                argv[0]);
        fprintf(stderr, "HOST_DBUS_SESSION_BUS_ADDRESS must point at the host session bus\n");
        return 64;
    }

    if (g_mkdir_with_parents(doc_dir, 0700) != 0) {
        fprintf(stderr, "create %s failed: %s\n", doc_dir, g_strerror(errno));
        return 1;
    }

    GError *error = NULL;
    BridgeState state = {
        .app_id = g_strdup(app_id),
        .doc_dir = g_strdup(doc_dir),
        .sandbox_root = g_strdup(sandbox_root),
        .mountpoint = g_strdup(mountpoint),
        .sandbox_doc_dirs = g_ptr_array_new_with_free_func(g_free),
        .grants = g_ptr_array_new_with_free_func((GDestroyNotify)free_grant),
        .requests = g_ptr_array_new_with_free_func((GDestroyNotify)free_request),
        .sessions = g_ptr_array_new_with_free_func((GDestroyNotify)free_session),
        .status_items = g_ptr_array_new_with_free_func((GDestroyNotify)free_status_item),
        .counter = 0,
        .request_counter = 0,
        .host_token_counter = 0,
        .status_counter = 0,
        .loop = g_main_loop_new(NULL, FALSE),
        .host_bus = connect_to_bus_address(host_bus_address, &error),
        .local_bus = NULL,
        .desktop_node = g_dbus_node_info_new_for_xml(DESKTOP_XML, &error),
        .documents_node = NULL,
        .request_node = NULL,
        .session_node = NULL,
        .status_watcher_node = NULL,
        .status_item_node = NULL,
        .dbusmenu_node = NULL,
        .control_node = NULL,
        .pipewire = NULL,
    };
    if (state.host_bus == NULL || state.desktop_node == NULL) {
        fprintf(stderr, "portal bridge setup failed: %s\n", error->message);
        g_error_free(error);
        return 1;
    }
    state.documents_node = g_dbus_node_info_new_for_xml(DOCUMENTS_XML, &error);
    state.request_node = g_dbus_node_info_new_for_xml(REQUEST_XML, &error);
    state.session_node = g_dbus_node_info_new_for_xml(SESSION_XML, &error);
    state.status_watcher_node = g_dbus_node_info_new_for_xml(STATUS_WATCHER_XML, &error);
    state.status_item_node = g_dbus_node_info_new_for_xml(STATUS_ITEM_XML, &error);
    state.dbusmenu_node = g_dbus_node_info_new_for_xml(DBUSMENU_XML, &error);
    state.control_node = g_dbus_node_info_new_for_xml(CONTROL_XML, &error);
    if (state.documents_node == NULL || state.request_node == NULL ||
        state.session_node == NULL ||
        state.status_watcher_node == NULL || state.status_item_node == NULL ||
        state.dbusmenu_node == NULL || state.control_node == NULL) {
        fprintf(stderr, "portal bridge introspection failed: %s\n", error->message);
        g_error_free(error);
        return 1;
    }
    load_host_screencast_properties(&state);
    state.pipewire = new_pipewire_compat(&state);
    if (state.pipewire == NULL) {
        log_line("PipeWire compatibility linking unavailable; ScreenCast forwarding will continue");
    }

    g_unix_signal_add(SIGINT, handle_signal, &state);
    g_unix_signal_add(SIGTERM, handle_signal, &state);

    guint desktop_owner_id =
        g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.portal.Desktop",
                       G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT |
                           G_BUS_NAME_OWNER_FLAGS_REPLACE,
                       on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
    guint documents_owner_id =
        g_bus_own_name(G_BUS_TYPE_SESSION, "org.freedesktop.portal.Documents",
                       G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT |
                           G_BUS_NAME_OWNER_FLAGS_REPLACE,
                       on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
    guint status_owner_id =
        g_bus_own_name(G_BUS_TYPE_SESSION, "org.kde.StatusNotifierWatcher",
                       G_BUS_NAME_OWNER_FLAGS_ALLOW_REPLACEMENT |
                           G_BUS_NAME_OWNER_FLAGS_REPLACE,
                       on_bus_acquired, on_name_acquired, on_name_lost, &state, NULL);
    log_line("serving private portal for %s at %s", state.app_id, state.doc_dir);
    g_main_loop_run(state.loop);

    cleanup_all(&state);
    for (guint i = 0; i < state.requests->len; i++) {
        RequestRecord *request = g_ptr_array_index(state.requests, i);
        if (!request->completed && request->host_path != NULL) {
            g_dbus_connection_call(state.host_bus,
                                   "org.freedesktop.portal.Desktop",
                                   request->host_path,
                                   "org.freedesktop.portal.Request", "Close",
                                   NULL, NULL, G_DBUS_CALL_FLAGS_NONE, -1,
                                   NULL, NULL, NULL);
        }
    }
    for (guint i = 0; i < state.sessions->len; i++) {
        close_host_session(g_ptr_array_index(state.sessions, i));
    }
    g_dbus_connection_flush_sync(state.host_bus, NULL, NULL);
    if (state.local_name_signal_id != 0 && state.local_bus != NULL) {
        g_dbus_connection_signal_unsubscribe(state.local_bus,
                                             state.local_name_signal_id);
        state.local_name_signal_id = 0;
    }
    free_pipewire_compat(state.pipewire);
    state.pipewire = NULL;
    g_ptr_array_free(state.status_items, TRUE);
    state.status_items = NULL;
    g_ptr_array_free(state.sessions, TRUE);
    state.sessions = NULL;
    g_ptr_array_free(state.requests, TRUE);
    state.requests = NULL;
    g_bus_unown_name(status_owner_id);
    g_bus_unown_name(documents_owner_id);
    g_bus_unown_name(desktop_owner_id);
    if (state.local_bus != NULL) {
        g_object_unref(state.local_bus);
    }
    if (state.host_bus != NULL) {
        g_object_unref(state.host_bus);
    }
    g_dbus_node_info_unref(state.desktop_node);
    g_dbus_node_info_unref(state.documents_node);
    g_dbus_node_info_unref(state.request_node);
    g_dbus_node_info_unref(state.session_node);
    g_dbus_node_info_unref(state.status_watcher_node);
    g_dbus_node_info_unref(state.status_item_node);
    g_dbus_node_info_unref(state.dbusmenu_node);
    g_dbus_node_info_unref(state.control_node);
    g_main_loop_unref(state.loop);
    g_ptr_array_free(state.grants, TRUE);
    g_ptr_array_free(state.sandbox_doc_dirs, TRUE);
    g_free(state.app_id);
    g_free(state.doc_dir);
    g_free(state.sandbox_root);
    g_free(state.mountpoint);
    return 0;
}
