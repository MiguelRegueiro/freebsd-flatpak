#define _GNU_SOURCE
#include "spawn_agent.h"
#include <errno.h>
#include <fcntl.h>
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define SPAWN_AGENT_INTERFACE "org.freebsd.Flatpak.SpawnAgent"
#define SPAWN_AGENT_PATH "/org/freebsd/Flatpak/SpawnAgent"
#define FLATPAK_PORTAL_BUS_NAME "org.freedesktop.portal.Flatpak"

enum {
  SPAWN_FLAG_CLEAR_ENV = 1u << 0,
  SPAWN_FLAG_WATCH_BUS = 1u << 4,
  SPAWN_FLAGS_IMPLEMENTED = SPAWN_FLAG_CLEAR_ENV | SPAWN_FLAG_WATCH_BUS
};

typedef struct {
  GMainLoop *loop;
  GDBusConnection *bus;
  GDBusNodeInfo *node;
  GPtrArray *children;
  char *portal_owner;
} SpawnAgent;

typedef struct {
  SpawnAgent *agent;
  GPid pid;
} AgentChild;

static const char *SPAWN_AGENT_XML =
    "<node>"
    " <interface name='org.freebsd.Flatpak.SpawnAgent'>"
    "  <method name='Spawn'>"
    "   <annotation name='org.gtk.GDBus.C.UnixFD' value='true'/>"
    "   <arg type='ay' direction='in'/><arg type='aay' direction='in'/>"
    "   <arg type='a{uh}' direction='in'/><arg type='a{ss}' direction='in'/>"
    "   <arg type='u' direction='in'/><arg type='a{sv}' direction='in'/>"
    "   <arg type='u' direction='out'/>"
    "  </method>"
    "  <method name='SpawnSignal'>"
    "   <arg type='u' direction='in'/><arg type='u' direction='in'/>"
    "   <arg type='b' direction='in'/>"
    "  </method>"
    "  <signal name='SpawnExited'>"
    "   <arg type='u'/><arg type='u'/>"
    "  </signal>"
    " </interface>"
    "</node>";

static AgentChild *find_child(SpawnAgent *agent, guint32 pid) {
  for (guint i = 0; i < agent->children->len; i++) {
    AgentChild *child = g_ptr_array_index(agent->children, i);
    if ((guint32)child->pid == pid)
      return child;
  }
  return NULL;
}

static char *bytestring_from_variant(GVariant *value, const char *label,
                                     GError **error) {
  gsize length = 0;
  const guint8 *bytes = g_variant_get_fixed_array(value, &length, 1);
  if (length == 0 || bytes[length - 1] != '\0' ||
      memchr(bytes, '\0', length - 1) != NULL) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "%s is not a valid bytestring", label);
    return NULL;
  }
  return g_strndup((const char *)bytes, length - 1);
}

static bool safe_absolute_cwd(const char *cwd) {
  if (cwd[0] != '/')
    return false;
  char **parts = g_strsplit(cwd, "/", -1);
  bool safe = true;
  for (guint i = 0; parts[i] != NULL; i++) {
    if (g_strcmp0(parts[i], "..") == 0) {
      safe = false;
      break;
    }
  }
  g_strfreev(parts);
  return safe;
}

static char **argv_from_variant(GVariant *value, GError **error) {
  gsize count = g_variant_n_children(value);
  if (count == 0) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "No command given");
    return NULL;
  }
  char **argv = g_new0(char *, count + 1);
  for (gsize i = 0; i < count; i++) {
    GVariant *item = g_variant_get_child_value(value, i);
    argv[i] = bytestring_from_variant(item, "argv entry", error);
    g_variant_unref(item);
    if (argv[i] == NULL || (i == 0 && argv[i][0] == '\0')) {
      if (argv[i] != NULL)
        g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                    "command must not be empty");
      g_strfreev(argv);
      return NULL;
    }
  }
  return argv;
}

static bool option_is_unsupported(const char *name) {
  static const char *known[] = {
      "sandbox-expose",       "sandbox-expose-ro", "sandbox-expose-fd",
      "sandbox-expose-fd-ro", "sandbox-flags",     "sandbox-a11y-own-names",
      "unset-env",            "usr-fd",            "app-fd",
      NULL};
  return g_strv_contains(known, name);
}

static char **build_environment(GVariant *envs, GVariant *options,
                                guint32 flags, GError **error) {
  GVariantIter option_iter;
  const char *option_name = NULL;
  GVariant *option_value = NULL;
  g_variant_iter_init(&option_iter, options);
  while (g_variant_iter_next(&option_iter, "{&sv}", &option_name,
                             &option_value)) {
    bool unsupported = option_is_unsupported(option_name);
    g_variant_unref(option_value);
    if (unsupported) {
      g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                  "unsupported Spawn option: %s", option_name);
      return NULL;
    }
  }

  char **environment = (flags & SPAWN_FLAG_CLEAR_ENV) ?
                           g_new0(char *, 1) :
                           g_get_environ();
  GVariantIter env_iter;
  const char *name = NULL;
  const char *value = NULL;
  g_variant_iter_init(&env_iter, envs);
  while (g_variant_iter_next(&env_iter, "{&s&s}", &name, &value)) {
    if (name[0] == '\0' || strchr(name, '=') != NULL) {
      g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                  "invalid environment variable name");
      g_strfreev(environment);
      return NULL;
    }
    environment = g_environ_setenv(environment, name, value, TRUE);
  }
  return environment;
}

typedef struct {
  int source;
  int destination;
} AgentFdMap;

static GArray *build_fd_map(GDBusMethodInvocation *invocation,
                            GVariant *mapping, GError **error) {
  GUnixFDList *fd_list = g_dbus_message_get_unix_fd_list(
      g_dbus_method_invocation_get_message(invocation));
  gint received_count = 0;
  const gint *received =
      fd_list == NULL ? NULL : g_unix_fd_list_peek_fds(fd_list, &received_count);
  GArray *handles = g_array_new(FALSE, FALSE, sizeof(gint32));
  GArray *destinations = g_array_new(FALSE, FALSE, sizeof(guint32));
  GVariantIter iter;
  guint32 destination = 0;
  gint32 handle = -1;
  g_variant_iter_init(&iter, mapping);
  while (g_variant_iter_next(&iter, "{uh}", &destination, &handle)) {
    if (destination > G_MAXINT || handle < 0 || handle >= received_count) {
      g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                  "invalid file descriptor mapping");
      g_array_free(handles, TRUE);
      g_array_free(destinations, TRUE);
      return NULL;
    }
    for (guint i = 0; i < destinations->len; i++) {
      if (g_array_index(destinations, guint32, i) == destination) {
        g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                    "duplicate destination file descriptor %u", destination);
        g_array_free(handles, TRUE);
        g_array_free(destinations, TRUE);
        return NULL;
      }
    }
    g_array_append_val(handles, handle);
    g_array_append_val(destinations, destination);
  }
  guint32 minimum_source = 3;
  for (guint i = 0; i < destinations->len; i++)
    minimum_source = MAX(minimum_source,
                         g_array_index(destinations, guint32, i) + 1);
  GArray *map = g_array_new(FALSE, FALSE, sizeof(AgentFdMap));
  for (guint i = 0; i < handles->len; i++) {
    gint32 stored_handle = g_array_index(handles, gint32, i);
    guint32 stored_destination = g_array_index(destinations, guint32, i);
    int duplicate =
        fcntl(received[stored_handle], F_DUPFD_CLOEXEC, minimum_source);
    if (duplicate < 0) {
      g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                  "duplicate passed file descriptor: %s", g_strerror(errno));
      for (guint i = 0; i < map->len; i++)
        close(g_array_index(map, AgentFdMap, i).source);
      g_array_free(map, TRUE);
      g_array_free(handles, TRUE);
      g_array_free(destinations, TRUE);
      return NULL;
    }
    AgentFdMap entry = {.source = duplicate,
                        .destination = (int)stored_destination};
    g_array_append_val(map, entry);
    minimum_source = MAX(minimum_source, (guint32)duplicate + 1);
  }
  g_array_free(handles, TRUE);
  g_array_free(destinations, TRUE);
  return map;
}

static void child_exited(GPid pid, gint status, gpointer user_data) {
  AgentChild *child = user_data;
  SpawnAgent *agent = child->agent;
  g_dbus_connection_emit_signal(
      agent->bus, NULL, SPAWN_AGENT_PATH, SPAWN_AGENT_INTERFACE,
      "SpawnExited", g_variant_new("(uu)", (guint32)pid, (guint32)status),
      NULL);
  for (guint i = 0; i < agent->children->len; i++) {
    if (g_ptr_array_index(agent->children, i) == child) {
      g_ptr_array_remove_index(agent->children, i);
      break;
    }
  }
  g_spawn_close_pid(pid);
}

static void handle_spawn(SpawnAgent *agent, GVariant *parameters,
                         GDBusMethodInvocation *invocation) {
  GError *error = NULL;
  GVariant *cwd_value = g_variant_get_child_value(parameters, 0);
  GVariant *argv_value = g_variant_get_child_value(parameters, 1);
  GVariant *fds_value = g_variant_get_child_value(parameters, 2);
  GVariant *envs_value = g_variant_get_child_value(parameters, 3);
  GVariant *flags_value = g_variant_get_child_value(parameters, 4);
  GVariant *options_value = g_variant_get_child_value(parameters, 5);
  guint32 flags = g_variant_get_uint32(flags_value);
  char *cwd = bytestring_from_variant(cwd_value, "cwd", &error);
  char **argv = error == NULL ? argv_from_variant(argv_value, &error) : NULL;
  char **environment = NULL;
  GArray *fd_map = NULL;

  if (error == NULL && (flags & ~SPAWN_FLAGS_IMPLEMENTED) != 0)
    g_set_error(&error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "unsupported Spawn flags: 0x%x",
                flags & ~SPAWN_FLAGS_IMPLEMENTED);
  if (error == NULL && !safe_absolute_cwd(cwd))
    g_set_error(&error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "cwd must be an absolute normalized sandbox path");
  if (error == NULL)
    environment = build_environment(envs_value, options_value, flags, &error);
  if (error == NULL)
    fd_map = build_fd_map(invocation, fds_value, &error);

  if (error != NULL) {
    g_dbus_method_invocation_take_error(invocation, error);
  } else {
    GPid pid = fork();
    if (pid < 0) {
      g_dbus_method_invocation_return_error(
          invocation, G_IO_ERROR, g_io_error_from_errno(errno),
          "fork failed: %s", g_strerror(errno));
    } else if (pid == 0) {
      if (setpgid(0, 0) != 0 || chdir(cwd) != 0)
        _exit(126);
      for (guint i = 0; i < fd_map->len; i++) {
        AgentFdMap entry = g_array_index(fd_map, AgentFdMap, i);
        if (dup2(entry.source, entry.destination) < 0)
          _exit(126);
      }
      for (guint i = 0; i < fd_map->len; i++)
        close(g_array_index(fd_map, AgentFdMap, i).source);
      execvpe(argv[0], argv, environment);
      _exit(errno == ENOENT ? 127 : 126);
    } else {
      AgentChild *child = g_new0(AgentChild, 1);
      child->agent = agent;
      child->pid = pid;
      g_ptr_array_add(agent->children, child);
      g_child_watch_add(pid, child_exited, child);
      g_dbus_method_invocation_return_value(invocation,
                                            g_variant_new("(u)", (guint32)pid));
    }
  }
  if (fd_map != NULL) {
    for (guint i = 0; i < fd_map->len; i++)
      close(g_array_index(fd_map, AgentFdMap, i).source);
    g_array_free(fd_map, TRUE);
  }
  g_free(cwd);
  g_strfreev(argv);
  g_strfreev(environment);
  g_variant_unref(cwd_value);
  g_variant_unref(argv_value);
  g_variant_unref(fds_value);
  g_variant_unref(envs_value);
  g_variant_unref(flags_value);
  g_variant_unref(options_value);
}

static void handle_agent_method(GDBusConnection *connection,
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
  SpawnAgent *agent = user_data;
  if (g_strcmp0(sender, agent->portal_owner) != 0) {
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_ACCESS_DENIED,
        "only the Flatpak portal bridge may call the spawn agent");
    return;
  }
  if (g_strcmp0(method_name, "Spawn") == 0) {
    handle_spawn(agent, parameters, invocation);
    return;
  }
  if (g_strcmp0(method_name, "SpawnSignal") == 0) {
    guint32 pid = 0;
    guint32 signal_number = 0;
    gboolean process_group = false;
    g_variant_get(parameters, "(uub)", &pid, &signal_number, &process_group);
    AgentChild *child = find_child(agent, pid);
    if (child == NULL || signal_number == 0 || signal_number >= NSIG) {
      g_dbus_method_invocation_return_error(invocation, G_DBUS_ERROR,
                                            G_DBUS_ERROR_UNIX_PROCESS_ID_UNKNOWN,
                                            "No such pid or signal");
      return;
    }
    int result = process_group ? killpg(child->pid, (int)signal_number)
                               : kill(child->pid, (int)signal_number);
    if (result != 0) {
      g_dbus_method_invocation_return_error(
          invocation, G_IO_ERROR, g_io_error_from_errno(errno),
          "signal failed: %s", g_strerror(errno));
      return;
    }
    g_dbus_method_invocation_return_value(invocation, NULL);
    return;
  }
  g_dbus_method_invocation_return_error(invocation, G_DBUS_ERROR,
                                        G_DBUS_ERROR_UNKNOWN_METHOD,
                                        "Unknown spawn agent method");
}

static const GDBusInterfaceVTable AGENT_VTABLE = {
    .method_call = handle_agent_method,
};

static gboolean stop_agent(gpointer user_data) {
  SpawnAgent *agent = user_data;
  for (guint i = 0; i < agent->children->len; i++) {
    AgentChild *child = g_ptr_array_index(agent->children, i);
    killpg(child->pid, SIGKILL);
  }
  g_main_loop_quit(agent->loop);
  return G_SOURCE_REMOVE;
}

static void agent_bus_acquired(GDBusConnection *connection, const gchar *name,
                               gpointer user_data) {
  (void)name;
  SpawnAgent *agent = user_data;
  agent->bus = g_object_ref(connection);
  GError *error = NULL;
  GVariant *owner_reply = g_dbus_connection_call_sync(
      connection, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "GetNameOwner",
      g_variant_new("(s)", FLATPAK_PORTAL_BUS_NAME), G_VARIANT_TYPE("(s)"),
      G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  if (owner_reply == NULL) {
    fprintf(stderr, "find Flatpak portal bridge failed: %s\n", error->message);
    g_error_free(error);
    g_main_loop_quit(agent->loop);
    return;
  }
  const char *portal_owner = NULL;
  g_variant_get(owner_reply, "(&s)", &portal_owner);
  g_free(agent->portal_owner);
  agent->portal_owner = g_strdup(portal_owner);
  g_variant_unref(owner_reply);
  if (g_dbus_connection_register_object(
          connection, SPAWN_AGENT_PATH, agent->node->interfaces[0],
          &AGENT_VTABLE, agent, NULL, &error) == 0) {
    fprintf(stderr, "spawn agent registration failed: %s\n", error->message);
    g_error_free(error);
    g_main_loop_quit(agent->loop);
  }
}

static void agent_name_lost(GDBusConnection *connection, const gchar *name,
                            gpointer user_data) {
  (void)connection;
  (void)name;
  stop_agent(user_data);
}

int run_spawn_agent(const char *bus_name) {
  GError *error = NULL;
  SpawnAgent agent = {
      .loop = g_main_loop_new(NULL, FALSE),
      .node = g_dbus_node_info_new_for_xml(SPAWN_AGENT_XML, &error),
      .children = g_ptr_array_new_with_free_func(g_free),
  };
  if (agent.node == NULL) {
    fprintf(stderr, "spawn agent introspection failed: %s\n", error->message);
    g_error_free(error);
    return 1;
  }
  g_unix_signal_add(SIGINT, stop_agent, &agent);
  g_unix_signal_add(SIGTERM, stop_agent, &agent);
  guint owner = g_bus_own_name(G_BUS_TYPE_SESSION, bus_name,
                               G_BUS_NAME_OWNER_FLAGS_NONE, agent_bus_acquired,
                               NULL, agent_name_lost, &agent, NULL);
  g_main_loop_run(agent.loop);
  g_bus_unown_name(owner);
  if (agent.bus != NULL)
    g_object_unref(agent.bus);
  g_free(agent.portal_owner);
  g_dbus_node_info_unref(agent.node);
  g_ptr_array_free(agent.children, TRUE);
  g_main_loop_unref(agent.loop);
  return 0;
}
