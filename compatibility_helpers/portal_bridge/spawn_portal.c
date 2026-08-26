#include "spawn_portal.h"

enum { SPAWN_FLAG_WATCH_BUS = 1u << 4 };

struct _SpawnSandbox {
  char *root;
  char *agent_name;
  char *agent_owner;
};

struct _SpawnProcess {
  guint32 pid;
  char *client_sender;
  char *agent_owner;
  bool watch_bus;
};

typedef struct {
  BridgeState *state;
  GDBusMethodInvocation *invocation;
  char *client_sender;
  char *agent_owner;
  guint32 flags;
} SpawnCall;

const char *FLATPAK_PORTAL_XML =
    "<node>"
    " <interface name='org.freedesktop.portal.Flatpak'>"
    "  <property name='version' type='u' access='read'/>"
    "  <property name='supports' type='u' access='read'/>"
    "  <method name='Spawn'>"
    "   <annotation name='org.gtk.GDBus.C.UnixFD' value='true'/>"
    "   <arg type='ay' name='cwd_path' direction='in'/>"
    "   <arg type='aay' name='argv' direction='in'/>"
    "   <arg type='a{uh}' name='fds' direction='in'/>"
    "   <arg type='a{ss}' name='envs' direction='in'/>"
    "   <arg type='u' name='flags' direction='in'/>"
    "   <arg type='a{sv}' name='options' direction='in'/>"
    "   <arg type='u' name='pid' direction='out'/>"
    "  </method>"
    "  <method name='SpawnSignal'>"
    "   <arg type='u' name='pid' direction='in'/>"
    "   <arg type='u' name='signal' direction='in'/>"
    "   <arg type='b' name='to_process_group' direction='in'/>"
    "  </method>"
    "  <signal name='SpawnExited'>"
    "   <arg type='u' name='pid'/><arg type='u' name='exit_status'/>"
    "  </signal>"
    " </interface>"
    "</node>";

static void free_spawn_sandbox(gpointer data) {
  SpawnSandbox *sandbox = data;
  g_free(sandbox->root);
  g_free(sandbox->agent_name);
  g_free(sandbox->agent_owner);
  g_free(sandbox);
}

static void free_spawn_process(gpointer data) {
  SpawnProcess *process = data;
  g_free(process->client_sender);
  g_free(process->agent_owner);
  g_free(process);
}

static char *bus_name_owner(BridgeState *state, const char *name) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_sync(
      state->local_bus, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "GetNameOwner", g_variant_new("(s)", name),
      G_VARIANT_TYPE("(s)"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  if (reply == NULL) {
    if (error != NULL)
      g_error_free(error);
    return NULL;
  }
  const char *owner = NULL;
  g_variant_get(reply, "(&s)", &owner);
  char *copy = g_strdup(owner);
  g_variant_unref(reply);
  return copy;
}

static SpawnSandbox *sandbox_for_sender(BridgeState *state,
                                        const char *sender) {
  for (guint i = 0; i < state->spawn_sandboxes->len; i++) {
    SpawnSandbox *sandbox = g_ptr_array_index(state->spawn_sandboxes, i);
    if (!portal_bridge_process_name_has_root(state, sender, sandbox->root))
      continue;
    char *owner = bus_name_owner(state, sandbox->agent_name);
    bool agent_is_current = g_strcmp0(owner, sandbox->agent_owner) == 0;
    g_free(owner);
    if (agent_is_current)
      return sandbox;
  }
  return NULL;
}

static SpawnProcess *find_spawn_process(BridgeState *state, guint32 pid,
                                        const char *client_sender,
                                        const char *agent_owner) {
  for (guint i = 0; i < state->spawn_processes->len; i++) {
    SpawnProcess *process = g_ptr_array_index(state->spawn_processes, i);
    if (process->pid == pid &&
        g_strcmp0(process->client_sender, client_sender) == 0 &&
        g_strcmp0(process->agent_owner, agent_owner) == 0)
      return process;
  }
  return NULL;
}

bool spawn_portal_add_sandbox(BridgeState *state, const char *root,
                              const char *agent_name, GError **error) {
  char *canonical_base = realpath(state->documents.sandbox_root, NULL);
  char *canonical_root = realpath(root, NULL);
  if (canonical_base == NULL || canonical_root == NULL ||
      !g_str_has_prefix(canonical_root, canonical_base) ||
      canonical_root[strlen(canonical_base)] != '/') {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
                "spawn sandbox is outside %s", state->documents.sandbox_root);
    free(canonical_base);
    free(canonical_root);
    return false;
  }
  if (!g_dbus_is_name(agent_name) || g_dbus_is_unique_name(agent_name) ||
      !g_str_has_prefix(agent_name, "org.freebsd.Flatpak.SpawnAgent.")) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_INVALID_ARGUMENT,
                "invalid spawn agent bus name");
    free(canonical_base);
    free(canonical_root);
    return false;
  }
  char *agent_owner = bus_name_owner(state, agent_name);
  if (agent_owner == NULL || !portal_bridge_process_name_has_root(
                                 state, agent_owner, canonical_root)) {
    g_set_error(error, G_IO_ERROR, G_IO_ERROR_PERMISSION_DENIED,
                "spawn agent does not belong to its sandbox");
    free(canonical_base);
    free(canonical_root);
    g_free(agent_owner);
    return false;
  }
  spawn_portal_remove_sandbox(state, canonical_root);
  SpawnSandbox *sandbox = g_new0(SpawnSandbox, 1);
  sandbox->root = g_strdup(canonical_root);
  sandbox->agent_name = g_strdup(agent_name);
  sandbox->agent_owner = agent_owner;
  g_ptr_array_add(state->spawn_sandboxes, sandbox);
  log_line("attached spawn agent %s for %s", agent_name, canonical_root);
  free(canonical_base);
  free(canonical_root);
  return true;
}

void spawn_portal_remove_sandbox(BridgeState *state, const char *root) {
  for (guint i = state->spawn_sandboxes->len; i > 0; i--) {
    SpawnSandbox *sandbox = g_ptr_array_index(state->spawn_sandboxes, i - 1);
    if (g_strcmp0(sandbox->root, root) == 0) {
      for (guint j = state->spawn_processes->len; j > 0; j--) {
        SpawnProcess *process =
            g_ptr_array_index(state->spawn_processes, j - 1);
        if (g_strcmp0(process->agent_owner, sandbox->agent_owner) == 0)
          g_ptr_array_remove_index(state->spawn_processes, j - 1);
      }
      g_ptr_array_remove_index(state->spawn_sandboxes, i - 1);
    }
  }
}

static void spawn_call_free(SpawnCall *call) {
  g_object_unref(call->invocation);
  g_free(call->client_sender);
  g_free(call->agent_owner);
  g_free(call);
}

static void spawn_forwarded(GObject *source, GAsyncResult *result,
                            gpointer user_data) {
  SpawnCall *call = user_data;
  GError *error = NULL;
  GUnixFDList *out_fds = NULL;
  GVariant *reply = g_dbus_connection_call_with_unix_fd_list_finish(
      G_DBUS_CONNECTION(source), &out_fds, result, &error);
  if (out_fds != NULL)
    g_object_unref(out_fds);
  if (reply == NULL) {
    g_dbus_method_invocation_take_error(call->invocation, error);
    spawn_call_free(call);
    return;
  }
  guint32 pid = 0;
  g_variant_get(reply, "(u)", &pid);
  SpawnProcess *process = g_new0(SpawnProcess, 1);
  process->pid = pid;
  process->client_sender = g_strdup(call->client_sender);
  process->agent_owner = g_strdup(call->agent_owner);
  process->watch_bus = (call->flags & SPAWN_FLAG_WATCH_BUS) != 0;
  g_ptr_array_add(call->state->spawn_processes, process);
  g_dbus_method_invocation_return_value(call->invocation, reply);
  spawn_call_free(call);
}

static void forward_spawn(BridgeState *state, const char *sender,
                          GVariant *parameters,
                          GDBusMethodInvocation *invocation,
                          SpawnSandbox *sandbox) {
  GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
  GUnixFDList *fds = g_dbus_message_get_unix_fd_list(message);
  SpawnCall *call = g_new0(SpawnCall, 1);
  call->state = state;
  call->invocation = g_object_ref(invocation);
  call->client_sender = g_strdup(sender);
  call->agent_owner = g_strdup(sandbox->agent_owner);
  GVariant *flags = g_variant_get_child_value(parameters, 4);
  call->flags = g_variant_get_uint32(flags);
  g_variant_unref(flags);
  g_dbus_connection_call_with_unix_fd_list(
      state->local_bus, sandbox->agent_owner, SPAWN_AGENT_PATH,
      SPAWN_AGENT_INTERFACE, "Spawn", g_variant_ref(parameters),
      G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, -1, fds, NULL,
      spawn_forwarded, call);
}

static void signal_forwarded(GObject *source, GAsyncResult *result,
                             gpointer user_data) {
  (void)source;
  GDBusMethodInvocation *invocation = user_data;
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_finish(
      g_dbus_method_invocation_get_connection(invocation), result, &error);
  if (reply == NULL)
    g_dbus_method_invocation_take_error(invocation, error);
  else
    g_dbus_method_invocation_return_value(invocation, reply);
  g_object_unref(invocation);
}

static void handle_flatpak_method(GDBusConnection *connection,
                                  const gchar *sender,
                                  const gchar *object_path,
                                  const gchar *interface_name,
                                  const gchar *method_name,
                                  GVariant *parameters,
                                  GDBusMethodInvocation *invocation,
                                  gpointer user_data) {
  (void)connection;
  (void)object_path;
  (void)interface_name;
  BridgeState *state = user_data;
  log_line("Flatpak.%s from %s", method_name, sender);
  SpawnSandbox *sandbox = sandbox_for_sender(state, sender);
  if (sandbox == NULL) {
    log_line("rejected Flatpak.%s from unregistered sandbox caller %s",
             method_name, sender);
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_ACCESS_DENIED,
        "caller does not belong to a registered application sandbox");
    return;
  }
  if (g_strcmp0(method_name, "Spawn") == 0) {
    forward_spawn(state, sender, parameters, invocation, sandbox);
    return;
  }
  if (g_strcmp0(method_name, "SpawnSignal") == 0) {
    guint32 pid = 0;
    guint32 signal_number = 0;
    gboolean process_group = false;
    g_variant_get(parameters, "(uub)", &pid, &signal_number, &process_group);
    SpawnProcess *process =
        find_spawn_process(state, pid, sender, sandbox->agent_owner);
    if (process == NULL) {
      g_dbus_method_invocation_return_error(invocation, G_DBUS_ERROR,
                                            G_DBUS_ERROR_UNIX_PROCESS_ID_UNKNOWN,
                                            "No such pid");
      return;
    }
    g_dbus_connection_call(state->local_bus, process->agent_owner,
                           SPAWN_AGENT_PATH, SPAWN_AGENT_INTERFACE,
                           "SpawnSignal", g_variant_ref(parameters),
                           G_VARIANT_TYPE_UNIT, G_DBUS_CALL_FLAGS_NONE, -1,
                           NULL, signal_forwarded, g_object_ref(invocation));
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_SUPPORTED,
                                        "%s is not implemented", method_name);
}

static GVariant *get_flatpak_property(GDBusConnection *connection,
                                      const gchar *sender,
                                      const gchar *object_path,
                                      const gchar *interface_name,
                                      const gchar *property_name,
                                      GError **error, gpointer user_data) {
  (void)connection;
  (void)object_path;
  (void)interface_name;
  BridgeState *state = user_data;
  if (sandbox_for_sender(state, sender) == NULL) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_ACCESS_DENIED,
                "caller does not belong to a registered application sandbox");
    return NULL;
  }
  if (g_strcmp0(property_name, "version") == 0)
    return g_variant_new_uint32(1);
  if (g_strcmp0(property_name, "supports") == 0)
    return g_variant_new_uint32(0);
  g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_PROPERTY,
              "unknown property %s", property_name);
  return NULL;
}

const GDBusInterfaceVTable FLATPAK_PORTAL_VTABLE = {
    .method_call = handle_flatpak_method,
    .get_property = get_flatpak_property,
};

static void agent_signal(GDBusConnection *connection, const gchar *sender_name,
                         const gchar *object_path,
                         const gchar *interface_name,
                         const gchar *signal_name, GVariant *parameters,
                         gpointer user_data) {
  (void)connection;
  (void)object_path;
  (void)interface_name;
  BridgeState *state = user_data;
  if (g_strcmp0(signal_name, "SpawnExited") != 0)
    return;
  guint32 pid = 0;
  guint32 status = 0;
  g_variant_get(parameters, "(uu)", &pid, &status);
  for (guint i = 0; i < state->spawn_processes->len; i++) {
    SpawnProcess *process = g_ptr_array_index(state->spawn_processes, i);
    if (process->pid == pid &&
        g_strcmp0(process->agent_owner, sender_name) == 0) {
      g_dbus_connection_emit_signal(
          state->local_bus, process->client_sender, FLATPAK_PORTAL_PATH,
          FLATPAK_PORTAL_INTERFACE, "SpawnExited",
          g_variant_new("(uu)", pid, status), NULL);
      g_ptr_array_remove_index(state->spawn_processes, i);
      return;
    }
  }
}

void spawn_portal_subscribe_agent_signals(BridgeState *state) {
  state->spawn_agent_signal_id = g_dbus_connection_signal_subscribe(
      state->local_bus, NULL, SPAWN_AGENT_INTERFACE, "SpawnExited",
      SPAWN_AGENT_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE, agent_signal, state,
      NULL);
}

void spawn_portal_close_client(BridgeState *state, const char *sender) {
  for (guint i = state->spawn_processes->len; i > 0; i--) {
    SpawnProcess *process = g_ptr_array_index(state->spawn_processes, i - 1);
    if (process->watch_bus &&
        g_strcmp0(process->client_sender, sender) == 0) {
      g_dbus_connection_call(
          state->local_bus, process->agent_owner, SPAWN_AGENT_PATH,
          SPAWN_AGENT_INTERFACE, "SpawnSignal",
          g_variant_new("(uub)", process->pid, (guint32)SIGKILL, TRUE), NULL,
          G_DBUS_CALL_FLAGS_NONE, -1, NULL, NULL, NULL);
    }
  }
}

void spawn_portal_cleanup(BridgeState *state) {
  if (state->spawn_agent_signal_id != 0 && state->local_bus != NULL)
    g_dbus_connection_signal_unsubscribe(state->local_bus,
                                         state->spawn_agent_signal_id);
  if (state->spawn_sandboxes != NULL)
    g_ptr_array_free(state->spawn_sandboxes, TRUE);
  if (state->spawn_processes != NULL)
    g_ptr_array_free(state->spawn_processes, TRUE);
  state->spawn_sandboxes = NULL;
  state->spawn_processes = NULL;
}

void spawn_portal_initialize(BridgeState *state) {
  state->spawn_sandboxes =
      g_ptr_array_new_with_free_func(free_spawn_sandbox);
  state->spawn_processes =
      g_ptr_array_new_with_free_func(free_spawn_process);
}
