#include "host_command.h"

#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <pthread.h>
#include <signal.h>
#include <stdint.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <unistd.h>

#define HOST_COMMAND_CLEAR_ENV (1u << 0)
#define HOST_COMMAND_WATCH_BUS (1u << 1)
#define HOST_COMMAND_SUPPORTED_FLAGS                                           \
  (HOST_COMMAND_CLEAR_ENV | HOST_COMMAND_WATCH_BUS)
#define FLATPAK_DEVELOPMENT_PATH "/org/freedesktop/Flatpak/Development"
#define FLATPAK_DEVELOPMENT_INTERFACE "org.freedesktop.Flatpak.Development"

typedef struct {
  int source;
  int target;
} FdMapping;

typedef struct {
  HostCommandService *service;
  GPid pid;
  char *client;
  guint child_watch_id;
  bool watch_bus;
} HostCommandRecord;

typedef struct {
  FdMapping *mappings;
  gsize mapping_count;
  int tty_source;
} ChildSetup;

const char HOST_COMMAND_XML[] =
    "<node>"
    " <interface name='org.freedesktop.Flatpak.Development'>"
    "  <property name='version' type='u' access='read'/>"
    "  <method name='HostCommand'>"
    "   <arg type='ay' direction='in' name='cwd_path'/>"
    "   <arg type='aay' direction='in' name='argv'/>"
    "   <arg type='a{uh}' direction='in' name='fds'/>"
    "   <arg type='a{ss}' direction='in' name='envs'/>"
    "   <arg type='u' direction='in' name='flags'/>"
    "   <arg type='u' direction='out' name='pid'/>"
    "  </method>"
    "  <method name='HostCommandSignal'>"
    "   <arg type='u' direction='in' name='pid'/>"
    "   <arg type='u' direction='in' name='signal'/>"
    "   <arg type='b' direction='in' name='to_process_group'/>"
    "  </method>"
    "  <signal name='HostCommandExited'>"
    "   <arg type='u' name='pid'/>"
    "   <arg type='u' name='exit_status'/>"
    "  </signal>"
    " </interface>"
    "</node>";

static void free_record(HostCommandRecord *record) {
  g_free(record->client);
  g_free(record);
}

static bool target_is_forwarded(FdMapping *mappings, gsize count, int fd) {
  for (gsize i = 0; i < count; i++) {
    if (mappings[i].target == fd) {
      return true;
    }
  }
  return false;
}

static void close_mappings(FdMapping *mappings, gsize count) {
  for (gsize i = 0; i < count; i++) {
    if (mappings[i].source >= 0) {
      close(mappings[i].source);
    }
  }
}

static bool prepare_fd_mappings(GVariant *fd_map, GUnixFDList *fd_list,
                                FdMapping **out_mappings, gsize *out_count,
                                int *out_tty_source, GError **error) {
  gsize count = g_variant_n_children(fd_map);
  FdMapping *mappings = g_new0(FdMapping, count);
  for (gsize i = 0; i < count; i++) {
    mappings[i].source = -1;
  }

  for (gsize i = 0; i < count; i++) {
    guint32 target;
    gint32 handle;
    g_variant_get_child(fd_map, i, "{uh}", &target, &handle);
    if (target > G_MAXINT || fd_list == NULL) {
      g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                  "Invalid file descriptor mapping");
      close_mappings(mappings, count);
      g_free(mappings);
      return false;
    }
    mappings[i].source = g_unix_fd_list_get(fd_list, handle, error);
    if (mappings[i].source < 0) {
      close_mappings(mappings, count);
      g_free(mappings);
      return false;
    }
    mappings[i].target = (int)target;
  }

  /* Every source must be outside the target set. This makes the child-side
   * dup2 sequence collision-free, including cycles such as 3->4, 4->3. */
  for (gsize i = 0; i < count; i++) {
    if (!target_is_forwarded(mappings, count, mappings[i].source)) {
      continue;
    }
    GArray *skipped = g_array_new(FALSE, FALSE, sizeof(int));
    int replacement;
    do {
      replacement = fcntl(mappings[i].source, F_DUPFD_CLOEXEC, 3);
      if (replacement < 0) {
        g_set_error(error, G_IO_ERROR, g_io_error_from_errno(errno),
                    "Duplicating forwarded file descriptor failed: %s",
                    g_strerror(errno));
        for (guint j = 0; j < skipped->len; j++) {
          close(g_array_index(skipped, int, j));
        }
        g_array_free(skipped, TRUE);
        close_mappings(mappings, count);
        g_free(mappings);
        return false;
      }
      if (target_is_forwarded(mappings, count, replacement)) {
        g_array_append_val(skipped, replacement);
      }
    } while (target_is_forwarded(mappings, count, replacement));
    close(mappings[i].source);
    mappings[i].source = replacement;
    for (guint j = 0; j < skipped->len; j++) {
      close(g_array_index(skipped, int, j));
    }
    g_array_free(skipped, TRUE);
  }

  *out_tty_source = -1;
  for (gsize i = 0; i < count; i++) {
    if (mappings[i].target <= STDERR_FILENO &&
        isatty(mappings[i].source)) {
      *out_tty_source = mappings[i].source;
      break;
    }
  }
  *out_mappings = mappings;
  *out_count = count;
  return true;
}

static void child_setup(gpointer user_data) {
  ChildSetup *setup = user_data;
  sigset_t empty;
  sigemptyset(&empty);
  pthread_sigmask(SIG_SETMASK, &empty, NULL);
  for (int signal_number = 1; signal_number < NSIG; signal_number++) {
    if (signal_number != SIGKILL && signal_number != SIGSTOP) {
      signal(signal_number, SIG_DFL);
    }
  }
  for (gsize i = 0; i < setup->mapping_count; i++) {
    if (dup2(setup->mappings[i].source, setup->mappings[i].target) < 0) {
      _exit(127);
    }
  }
  for (gsize i = 0; i < setup->mapping_count; i++) {
    close(setup->mappings[i].source);
  }
  setsid();
  setpgid(0, 0);
  if (setup->tty_source >= 0) {
    for (gsize i = 0; i < setup->mapping_count; i++) {
      if (setup->mappings[i].source == setup->tty_source) {
        ioctl(setup->mappings[i].target, TIOCSCTTY, 0);
        break;
      }
    }
  }
}

static char *dup_byte_string(GVariant *value, bool allow_empty,
                             GError **error) {
  gsize length = 0;
  const guint8 *bytes = g_variant_get_fixed_array(value, &length, 1);
  if (length > 0 && bytes[length - 1] == 0) {
    length--;
  }
  if ((!allow_empty && length == 0) ||
      (length > 0 && memchr(bytes, 0, length) != NULL)) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "Byte strings must contain one non-empty command string");
    return NULL;
  }
  return g_strndup((const char *)bytes, length);
}

static char **dup_argv(GVariant *argv_variant, GError **error) {
  gsize count = g_variant_n_children(argv_variant);
  if (count == 0) {
    g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "No command given");
    return NULL;
  }
  char **argv = g_new0(char *, count + 1);
  for (gsize i = 0; i < count; i++) {
    GVariant *item = g_variant_get_child_value(argv_variant, i);
    argv[i] = dup_byte_string(item, i != 0, error);
    g_variant_unref(item);
    if (argv[i] == NULL) {
      g_strfreev(argv);
      return NULL;
    }
  }
  return argv;
}

static char **build_environment(HostCommandService *service, GVariant *envs,
                                guint32 flags) {
  char **environment = (flags & HOST_COMMAND_CLEAR_ENV)
                           ? g_new0(char *, 1)
                           : g_strdupv(service->host_environment);
  GVariantIter iter;
  const char *name;
  const char *value;
  g_variant_iter_init(&iter, envs);
  while (g_variant_iter_next(&iter, "{&s&s}", &name, &value)) {
    environment = g_environ_setenv(environment, name, value, TRUE);
  }
  return environment;
}

static void command_exited(GPid pid, gint status, gpointer user_data) {
  HostCommandRecord *record = user_data;
  HostCommandService *service = record->service;
  record->child_watch_id = 0;
  g_dbus_connection_emit_signal(
      service->connection, record->client, FLATPAK_DEVELOPMENT_PATH,
      FLATPAK_DEVELOPMENT_INTERFACE, "HostCommandExited",
      g_variant_new("(uu)", (guint32)pid, (guint32)status), NULL);
  g_spawn_close_pid(pid);
  g_hash_table_remove(service->commands, GUINT_TO_POINTER((guint)pid));
}

static void return_spawn_error(GDBusMethodInvocation *invocation,
                               GError *error) {
  GDBusError code = G_DBUS_ERROR_FAILED;
  if (g_error_matches(error, G_SPAWN_ERROR, G_SPAWN_ERROR_ACCES)) {
    code = G_DBUS_ERROR_ACCESS_DENIED;
  } else if (g_error_matches(error, G_SPAWN_ERROR, G_SPAWN_ERROR_NOENT)) {
    code = G_DBUS_ERROR_FILE_NOT_FOUND;
  }
  g_dbus_method_invocation_return_error(invocation, G_DBUS_ERROR, code,
                                        "Failed to start command: %s",
                                        error->message);
}

static void handle_host_command(HostCommandService *service,
                                GVariant *parameters,
                                GDBusMethodInvocation *invocation) {
  GVariant *cwd_variant = NULL;
  GVariant *argv_variant = NULL;
  GVariant *fd_map = NULL;
  GVariant *envs = NULL;
  guint32 flags;
  g_variant_get(parameters, "(@ay@aay@a{uh}@a{ss}u)", &cwd_variant,
                &argv_variant, &fd_map, &envs, &flags);

  GError *error = NULL;
  char *cwd = dup_byte_string(cwd_variant, true, &error);
  char **argv = error == NULL ? dup_argv(argv_variant, &error) : NULL;
  if ((flags & ~HOST_COMMAND_SUPPORTED_FLAGS) != 0 && error == NULL) {
    g_set_error(&error, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
                "Unsupported HostCommand flags");
  }

  GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
  GUnixFDList *fd_list = g_dbus_message_get_unix_fd_list(message);
  FdMapping *mappings = NULL;
  gsize mapping_count = 0;
  int tty_source = -1;
  if (error == NULL && !prepare_fd_mappings(fd_map, fd_list, &mappings,
                                             &mapping_count, &tty_source,
                                             &error)) {
    /* error is returned below */
  }
  if (error != NULL) {
    g_dbus_method_invocation_return_gerror(invocation, error);
    g_error_free(error);
    g_free(cwd);
    g_strfreev(argv);
    g_variant_unref(cwd_variant);
    g_variant_unref(argv_variant);
    g_variant_unref(fd_map);
    g_variant_unref(envs);
    return;
  }

  char **environment = build_environment(service, envs, flags);
  ChildSetup setup = {mappings, mapping_count, tty_source};
  GPid pid;
  bool spawned = g_spawn_async(
      cwd[0] == '\0' ? NULL : cwd, argv, environment,
      G_SPAWN_SEARCH_PATH | G_SPAWN_DO_NOT_REAP_CHILD, child_setup, &setup,
      &pid, &error);
  close_mappings(mappings, mapping_count);
  g_free(mappings);
  g_strfreev(environment);
  g_free(cwd);
  g_strfreev(argv);
  g_variant_unref(cwd_variant);
  g_variant_unref(argv_variant);
  g_variant_unref(fd_map);
  g_variant_unref(envs);
  if (!spawned) {
    return_spawn_error(invocation, error);
    g_error_free(error);
    return;
  }

  HostCommandRecord *record = g_new0(HostCommandRecord, 1);
  record->service = service;
  record->pid = pid;
  record->client =
      g_strdup(g_dbus_method_invocation_get_sender(invocation));
  record->watch_bus = (flags & HOST_COMMAND_WATCH_BUS) != 0;
  g_hash_table_insert(service->commands, GUINT_TO_POINTER((guint)pid), record);
  record->child_watch_id = g_child_watch_add(pid, command_exited, record);
  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(u)", (guint32)pid));
}

static void handle_host_command_signal(HostCommandService *service,
                                       GVariant *parameters,
                                       GDBusMethodInvocation *invocation) {
  guint32 pid;
  guint32 signal_number;
  gboolean to_process_group;
  g_variant_get(parameters, "(uub)", &pid, &signal_number,
                &to_process_group);
  HostCommandRecord *record =
      g_hash_table_lookup(service->commands, GUINT_TO_POINTER(pid));
  if (record == NULL ||
      g_strcmp0(record->client,
                g_dbus_method_invocation_get_sender(invocation)) != 0) {
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_UNIX_PROCESS_ID_UNKNOWN,
        "No such pid for this caller");
    return;
  }
  if (signal_number == 0 || signal_number >= NSIG) {
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_INVALID_ARGS,
        "Invalid signal number");
    return;
  }
  int result = to_process_group ? killpg((pid_t)pid, (int)signal_number)
                                : kill((pid_t)pid, (int)signal_number);
  if (result < 0 && errno != ESRCH) {
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_FAILED,
        "Sending signal failed: %s", g_strerror(errno));
    return;
  }
  g_dbus_method_invocation_return_value(invocation, g_variant_new("()"));
}

static void on_method_call(GDBusConnection *connection, const gchar *sender,
                           const gchar *object_path,
                           const gchar *interface_name,
                           const gchar *method_name, GVariant *parameters,
                           GDBusMethodInvocation *invocation,
                           gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  HostCommandService *service = user_data;
  if (g_str_equal(method_name, "HostCommand")) {
    handle_host_command(service, parameters, invocation);
  } else if (g_str_equal(method_name, "HostCommandSignal")) {
    handle_host_command_signal(service, parameters, invocation);
  } else {
    g_dbus_method_invocation_return_error(
        invocation, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_METHOD,
        "Unknown development method %s", method_name);
  }
}

static GVariant *get_property(GDBusConnection *connection, const gchar *sender,
                              const gchar *object_path,
                              const gchar *interface_name,
                              const gchar *property_name, GError **error,
                              gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)user_data;
  if (g_str_equal(property_name, "version")) {
    return g_variant_new_uint32(1);
  }
  g_set_error(error, G_DBUS_ERROR, G_DBUS_ERROR_UNKNOWN_PROPERTY,
              "Unknown property %s", property_name);
  return NULL;
}

static const GDBusInterfaceVTable HOST_COMMAND_VTABLE = {
    .method_call = on_method_call,
    .get_property = get_property,
};

bool host_command_service_init(HostCommandService *service,
                               const char *host_bus_address, GError **error) {
  memset(service, 0, sizeof(*service));
  service->node = g_dbus_node_info_new_for_xml(HOST_COMMAND_XML, error);
  if (service->node == NULL) {
    return false;
  }
  service->commands = g_hash_table_new_full(
      g_direct_hash, g_direct_equal, NULL, (GDestroyNotify)free_record);
  service->host_environment = g_get_environ();
  service->host_environment = g_environ_setenv(
      service->host_environment, "DBUS_SESSION_BUS_ADDRESS", host_bus_address,
      TRUE);
  service->host_environment = g_environ_unsetenv(
      service->host_environment, "HOST_DBUS_SESSION_BUS_ADDRESS");
  return true;
}

bool host_command_service_register(HostCommandService *service,
                                   GDBusConnection *connection,
                                   GError **error) {
  if (service->registration_id != 0) {
    return true;
  }
  service->connection = g_object_ref(connection);
  service->registration_id = g_dbus_connection_register_object(
      connection, FLATPAK_DEVELOPMENT_PATH, service->node->interfaces[0],
      &HOST_COMMAND_VTABLE, service, NULL, error);
  return service->registration_id != 0;
}

void host_command_service_close_client(HostCommandService *service,
                                       const char *client_sender) {
  if (service->commands == NULL) {
    return;
  }
  GHashTableIter iter;
  gpointer value;
  g_hash_table_iter_init(&iter, service->commands);
  while (g_hash_table_iter_next(&iter, NULL, &value)) {
    HostCommandRecord *record = value;
    if (record->watch_bus &&
        g_strcmp0(record->client, client_sender) == 0) {
      killpg(record->pid, SIGINT);
    }
  }
}

void host_command_service_cleanup(HostCommandService *service) {
  if (service->commands == NULL) {
    return;
  }
  GHashTableIter iter;
  gpointer value;
  g_hash_table_iter_init(&iter, service->commands);
  while (g_hash_table_iter_next(&iter, NULL, &value)) {
    HostCommandRecord *record = value;
    if (record->child_watch_id != 0) {
      g_source_remove(record->child_watch_id);
      record->child_watch_id = 0;
    }
    killpg(record->pid, SIGTERM);
  }
  for (int attempt = 0; attempt < 20 && g_hash_table_size(service->commands) > 0;
       attempt++) {
    g_hash_table_iter_init(&iter, service->commands);
    while (g_hash_table_iter_next(&iter, NULL, &value)) {
      HostCommandRecord *record = value;
      int status;
      pid_t result = waitpid(record->pid, &status, WNOHANG);
      if (result == record->pid || (result < 0 && errno == ECHILD)) {
        g_hash_table_iter_remove(&iter);
      }
    }
    if (g_hash_table_size(service->commands) > 0) {
      g_usleep(100000);
    }
  }
  g_hash_table_iter_init(&iter, service->commands);
  while (g_hash_table_iter_next(&iter, NULL, &value)) {
    HostCommandRecord *record = value;
    killpg(record->pid, SIGKILL);
    waitpid(record->pid, NULL, 0);
    g_hash_table_iter_remove(&iter);
  }
}

void host_command_service_clear(HostCommandService *service) {
  host_command_service_cleanup(service);
  if (service->registration_id != 0 && service->connection != NULL) {
    g_dbus_connection_unregister_object(service->connection,
                                        service->registration_id);
  }
  if (service->commands != NULL) {
    g_hash_table_destroy(service->commands);
  }
  g_strfreev(service->host_environment);
  if (service->node != NULL) {
    g_dbus_node_info_unref(service->node);
  }
  if (service->connection != NULL) {
    g_object_unref(service->connection);
  }
  memset(service, 0, sizeof(*service));
}
