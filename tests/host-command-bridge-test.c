#include "../compatibility_helpers/portal_bridge/host_command.h"

#include <errno.h>
#include <gio/gio.h>
#include <gio/gunixfdlist.h>
#include <glib-unix.h>
#include <signal.h>
#include <stdbool.h>
#include <stdio.h>
#include <string.h>
#include <sys/wait.h>
#include <unistd.h>

#define SERVICE_NAME "org.freedesktop.Flatpak"
#define DEVELOPMENT_PATH "/org/freedesktop/Flatpak/Development"
#define DEVELOPMENT_INTERFACE "org.freedesktop.Flatpak.Development"

typedef struct {
  guint32 expected_pid;
  guint32 status;
  bool received;
} ExitSignal;

static gboolean quit_service(gpointer user_data) {
  g_main_loop_quit(user_data);
  return G_SOURCE_REMOVE;
}

static void service_name_changed(GDBusConnection *connection,
                                 const gchar *sender_name,
                                 const gchar *object_path,
                                 const gchar *interface_name,
                                 const gchar *signal_name, GVariant *parameters,
                                 gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  const char *name;
  const char *old_owner;
  const char *new_owner;
  g_variant_get(parameters, "(&s&s&s)", &name, &old_owner, &new_owner);
  if (name[0] == ':' && old_owner[0] != '\0' && new_owner[0] == '\0') {
    host_command_service_close_client(user_data, name);
  }
}

static int run_service(const char *address) {
  GError *error = NULL;
  GDBusConnection *connection = g_dbus_connection_new_for_address_sync(
      address,
      G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
          G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
      NULL, NULL, &error);
  if (connection == NULL) {
    g_printerr("test service connection failed: %s\n", error->message);
    return 1;
  }

  HostCommandService service;
  if (!host_command_service_init(&service, address, &error) ||
      !host_command_service_register(&service, connection, &error)) {
    g_printerr("test service setup failed: %s\n", error->message);
    return 1;
  }
  g_dbus_connection_signal_subscribe(
      connection, "org.freedesktop.DBus", "org.freedesktop.DBus",
      "NameOwnerChanged", "/org/freedesktop/DBus", NULL,
      G_DBUS_SIGNAL_FLAGS_NONE, service_name_changed, &service, NULL);
  GVariant *reply = g_dbus_connection_call_sync(
      connection, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "RequestName",
      g_variant_new("(su)", SERVICE_NAME, 0u), G_VARIANT_TYPE("(u)"),
      G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  if (reply == NULL) {
    g_printerr("test service name failed: %s\n", error->message);
    return 1;
  }
  guint32 request_result;
  g_variant_get(reply, "(u)", &request_result);
  g_variant_unref(reply);
  if (request_result != 1) {
    g_printerr("test service did not become primary owner\n");
    return 1;
  }

  GMainLoop *loop = g_main_loop_new(NULL, FALSE);
  g_unix_signal_add(SIGTERM, quit_service, loop);
  g_unix_signal_add(SIGINT, quit_service, loop);
  g_main_loop_run(loop);
  host_command_service_clear(&service);
  g_main_loop_unref(loop);
  g_object_unref(connection);
  return 0;
}

static GDBusConnection *new_connection(const char *address) {
  GError *error = NULL;
  GDBusConnection *connection = g_dbus_connection_new_for_address_sync(
      address,
      G_DBUS_CONNECTION_FLAGS_AUTHENTICATION_CLIENT |
          G_DBUS_CONNECTION_FLAGS_MESSAGE_BUS_CONNECTION,
      NULL, NULL, &error);
  g_assert_no_error(error);
  return connection;
}

static bool name_has_owner(GDBusConnection *connection) {
  GError *error = NULL;
  GVariant *reply = g_dbus_connection_call_sync(
      connection, "org.freedesktop.DBus", "/org/freedesktop/DBus",
      "org.freedesktop.DBus", "NameHasOwner",
      g_variant_new("(s)", SERVICE_NAME), G_VARIANT_TYPE("(b)"),
      G_DBUS_CALL_FLAGS_NONE, -1, NULL, &error);
  g_assert_no_error(error);
  gboolean owned;
  g_variant_get(reply, "(b)", &owned);
  g_variant_unref(reply);
  return owned;
}

static void exited_cb(GDBusConnection *connection, const gchar *sender_name,
                      const gchar *object_path, const gchar *interface_name,
                      const gchar *signal_name, GVariant *parameters,
                      gpointer user_data) {
  (void)connection;
  (void)sender_name;
  (void)object_path;
  (void)interface_name;
  (void)signal_name;
  ExitSignal *result = user_data;
  guint32 pid;
  guint32 status;
  g_variant_get(parameters, "(uu)", &pid, &status);
  if (pid == result->expected_pid) {
    result->status = status;
    result->received = true;
  }
}

static bool wait_for_exit_signal(ExitSignal *result) {
  gint64 deadline = g_get_monotonic_time() + 5 * G_TIME_SPAN_SECOND;
  while (!result->received && g_get_monotonic_time() < deadline) {
    while (g_main_context_iteration(NULL, FALSE)) {
    }
    g_usleep(10000);
  }
  return result->received;
}

static GVariant *new_argv(const char *const *argv) {
  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE("aay"));
  for (gsize i = 0; argv[i] != NULL; i++) {
    g_variant_builder_add_value(&builder, g_variant_new_bytestring(argv[i]));
  }
  return g_variant_builder_end(&builder);
}

static guint32 spawn_command(GDBusConnection *connection, const char *cwd,
                             const char *const *argv, GUnixFDList *fd_list,
                             GVariant *fd_map, GVariant *envs, guint32 flags,
                             GError **error) {
  GVariant *reply = g_dbus_connection_call_with_unix_fd_list_sync(
      connection, SERVICE_NAME, DEVELOPMENT_PATH, DEVELOPMENT_INTERFACE,
      "HostCommand",
      g_variant_new("(@ay@aay@a{uh}@a{ss}u)",
                    g_variant_new_bytestring(cwd), new_argv(argv), fd_map,
                    envs, flags),
      G_VARIANT_TYPE("(u)"), G_DBUS_CALL_FLAGS_NONE, -1, fd_list, NULL, NULL,
      error);
  if (reply == NULL) {
    return 0;
  }
  guint32 pid;
  g_variant_get(reply, "(u)", &pid);
  g_variant_unref(reply);
  return pid;
}

static GVariant *empty_fds(void) {
  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE("a{uh}"));
  return g_variant_builder_end(&builder);
}

static GVariant *empty_env(void) {
  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE("a{ss}"));
  return g_variant_builder_end(&builder);
}

static void test_spawn_contract(GDBusConnection *connection) {
  int output_pipe[2];
  int ignored_pipe[2];
  g_assert_cmpint(pipe(output_pipe), ==, 0);
  g_assert_cmpint(pipe(ignored_pipe), ==, 0);
  GError *error = NULL;
  GUnixFDList *fd_list = g_unix_fd_list_new();
  int ignored_handle =
      g_unix_fd_list_append(fd_list, ignored_pipe[0], &error);
  g_assert_no_error(error);
  g_assert_cmpint(ignored_handle, ==, 0);
  int handle = g_unix_fd_list_append(fd_list, output_pipe[1], &error);
  g_assert_no_error(error);
  close(output_pipe[1]);
  close(ignored_pipe[0]);
  close(ignored_pipe[1]);
  GVariantBuilder fd_builder;
  g_variant_builder_init(&fd_builder, G_VARIANT_TYPE("a{uh}"));
  g_variant_builder_add(&fd_builder, "{uh}", 1u, handle);
  GVariantBuilder env_builder;
  g_variant_builder_init(&env_builder, G_VARIANT_TYPE("a{ss}"));
  g_variant_builder_add(&env_builder, "{ss}", "HOST_COMMAND_TEST", "env-ok");
  const char *argv[] = {"/bin/sh", "-c",
                        "printf '%s|%s|%s' \"$PWD\" "
                        "\"$HOST_COMMAND_TEST\" \"$1\"; exit 7",
                        "sh", "argv-ok", NULL};

  ExitSignal exited = {0};
  guint subscription = g_dbus_connection_signal_subscribe(
      connection, NULL, DEVELOPMENT_INTERFACE, "HostCommandExited",
      DEVELOPMENT_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE, exited_cb, &exited,
      NULL);
  guint32 pid = spawn_command(
      connection, "/tmp", argv, fd_list, g_variant_builder_end(&fd_builder),
      g_variant_builder_end(&env_builder), 0, &error);
  g_assert_no_error(error);
  g_assert_cmpuint(pid, >, 0);
  exited.expected_pid = pid;
  g_assert_true(wait_for_exit_signal(&exited));
  g_assert_true(WIFEXITED((int)exited.status));
  g_assert_cmpint(WEXITSTATUS((int)exited.status), ==, 7);

  char output[128] = {0};
  ssize_t length = read(output_pipe[0], output, sizeof(output) - 1);
  g_assert_cmpint(length, >, 0);
  g_assert_cmpstr(output, ==, "/tmp|env-ok|argv-ok");
  close(output_pipe[0]);
  g_dbus_connection_signal_unsubscribe(connection, subscription);
  g_object_unref(fd_list);
}

static guint32 spawn_sleep(GDBusConnection *connection, guint32 flags) {
  const char *argv[] = {"/bin/sleep", "30", NULL};
  GError *error = NULL;
  guint32 pid = spawn_command(connection, "", argv, NULL, empty_fds(),
                              empty_env(), flags, &error);
  g_assert_no_error(error);
  g_assert_cmpuint(pid, >, 0);
  return pid;
}

static void signal_command(GDBusConnection *connection, guint32 pid,
                           guint32 signal_number, GError **error) {
  GVariant *reply = g_dbus_connection_call_sync(
      connection, SERVICE_NAME, DEVELOPMENT_PATH, DEVELOPMENT_INTERFACE,
      "HostCommandSignal", g_variant_new("(uub)", pid, signal_number, FALSE),
      G_VARIANT_TYPE("()"), G_DBUS_CALL_FLAGS_NONE, -1, NULL, error);
  if (reply != NULL) {
    g_variant_unref(reply);
  }
}

static void test_signal_caller_isolation(GDBusConnection *owner,
                                         GDBusConnection *other) {
  ExitSignal exited = {0};
  guint subscription = g_dbus_connection_signal_subscribe(
      owner, NULL, DEVELOPMENT_INTERFACE, "HostCommandExited",
      DEVELOPMENT_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE, exited_cb, &exited,
      NULL);
  exited.expected_pid = spawn_sleep(owner, 0);

  GError *error = NULL;
  signal_command(other, exited.expected_pid, SIGTERM, &error);
  g_assert_error(error, G_DBUS_ERROR, G_DBUS_ERROR_UNIX_PROCESS_ID_UNKNOWN);
  g_assert_nonnull(strstr(error->message, "No such pid for this caller"));
  g_clear_error(&error);
  g_assert_cmpint(kill((pid_t)exited.expected_pid, 0), ==, 0);

  signal_command(owner, exited.expected_pid, SIGTERM, &error);
  g_assert_no_error(error);
  g_assert_true(wait_for_exit_signal(&exited));
  g_assert_true(WIFSIGNALED((int)exited.status));
  g_assert_cmpint(WTERMSIG((int)exited.status), ==, SIGTERM);
  g_dbus_connection_signal_unsubscribe(owner, subscription);
}

static bool wait_until_process_gone(guint32 pid) {
  gint64 deadline = g_get_monotonic_time() + 5 * G_TIME_SPAN_SECOND;
  while (g_get_monotonic_time() < deadline) {
    if (kill((pid_t)pid, 0) < 0 && errno == ESRCH) {
      return true;
    }
    while (g_main_context_iteration(NULL, FALSE)) {
    }
    g_usleep(10000);
  }
  return false;
}

static void test_watch_bus(const char *address) {
  GDBusConnection *watched = new_connection(address);
  guint32 pid = spawn_sleep(watched, 2);
  GError *error = NULL;
  g_dbus_connection_close_sync(watched, NULL, &error);
  g_assert_no_error(error);
  g_object_unref(watched);
  g_assert_true(wait_until_process_gone(pid));
}

static void test_clear_environment(GDBusConnection *connection) {
  const char *argv[] = {"/bin/sh", "-c",
                        "test -z \"${HOST_COMMAND_PARENT_SENTINEL+x}\"",
                        NULL};
  ExitSignal exited = {0};
  guint subscription = g_dbus_connection_signal_subscribe(
      connection, NULL, DEVELOPMENT_INTERFACE, "HostCommandExited",
      DEVELOPMENT_PATH, NULL, G_DBUS_SIGNAL_FLAGS_NONE, exited_cb, &exited,
      NULL);
  GError *error = NULL;
  exited.expected_pid = spawn_command(connection, "", argv, NULL, empty_fds(),
                                      empty_env(), 1, &error);
  g_assert_no_error(error);
  g_assert_true(wait_for_exit_signal(&exited));
  g_assert_true(WIFEXITED((int)exited.status));
  g_assert_cmpint(WEXITSTATUS((int)exited.status), ==, 0);
  g_dbus_connection_signal_unsubscribe(connection, subscription);
}

int main(int argc, char **argv) {
  if (argc == 3 && strcmp(argv[1], "--service") == 0) {
    return run_service(argv[2]);
  }

  g_setenv("HOST_COMMAND_PARENT_SENTINEL", "present", TRUE);
  GTestDBus *test_bus = g_test_dbus_new(G_TEST_DBUS_NONE);
  g_test_dbus_up(test_bus);
  const char *address = g_test_dbus_get_bus_address(test_bus);
  GDBusConnection *first = new_connection(address);
  GDBusConnection *second = new_connection(address);

  /* This is the denied state: without an explicitly launched, permission-
   * gated bridge, the private app bus has no Flatpak development service. */
  g_assert_false(name_has_owner(first));

  char *service_argv[] = {argv[0], "--service", (char *)address, NULL};
  GError *error = NULL;
  GPid service_pid;
  g_assert_true(g_spawn_async(NULL, service_argv, NULL,
                              G_SPAWN_DO_NOT_REAP_CHILD, NULL, NULL,
                              &service_pid, &error));
  g_assert_no_error(error);
  gint64 deadline = g_get_monotonic_time() + 5 * G_TIME_SPAN_SECOND;
  while (!name_has_owner(first) && g_get_monotonic_time() < deadline) {
    g_usleep(10000);
  }
  g_assert_true(name_has_owner(first));

  test_spawn_contract(first);
  test_clear_environment(first);
  test_signal_caller_isolation(first, second);
  test_watch_bus(address);

  guint32 cleanup_pid = spawn_sleep(first, 0);
  g_assert_cmpint(kill(service_pid, SIGTERM), ==, 0);
  int service_status;
  g_assert_cmpint(waitpid(service_pid, &service_status, 0), ==, service_pid);
  g_assert_true(WIFEXITED(service_status));
  g_assert_cmpint(WEXITSTATUS(service_status), ==, 0);
  g_assert_true(wait_until_process_gone(cleanup_pid));

  g_object_unref(second);
  g_object_unref(first);
  g_test_dbus_down(test_bus);
  g_object_unref(test_bus);
  return 0;
}
