#include "../compatibility_helpers/portal_bridge/basic_desktop_portals.h"
#include "../compatibility_helpers/portal_bridge/flatpak_spawn_portal.h"
#include "../compatibility_helpers/portal_bridge/document_grant_store.h"
#include "../compatibility_helpers/portal_bridge/document_grant_persistence.h"
#include "../compatibility_helpers/portal_bridge/document_mounts.h"
#include "../compatibility_helpers/portal_bridge/document_id.h"
#include "../compatibility_helpers/portal_bridge/document_portal.h"
#include "../compatibility_helpers/portal_bridge/file_chooser_portal.h"
#include "../compatibility_helpers/portal_bridge/pipewire_screencast_linker.h"
#include "../compatibility_helpers/portal_bridge/portal_request.h"
#include "../compatibility_helpers/portal_bridge/sandbox_document_registration.h"
#include "../compatibility_helpers/portal_bridge/screencast_portal.h"
#include "../compatibility_helpers/status_notifier_bridge/status_notifier_watcher.h"
#include "../compatibility_helpers/status_notifier_bridge/icon_resolver.h"
#include <arpa/inet.h>
#include <gdk-pixbuf/gdk-pixbuf.h>

static bool test_unmount_succeeds = true;
typedef struct {
  char *source;
  char *target;
  bool read_only;
} TestMountCall;

static GPtrArray *test_mount_calls;

static char *read_test_stream(FILE *stream) {
  g_assert_cmpint(fflush(stream), ==, 0);
  g_assert_cmpint(fseek(stream, 0, SEEK_END), ==, 0);
  long length = ftell(stream);
  g_assert_cmpint(length, >=, 0);
  g_assert_cmpint(fseek(stream, 0, SEEK_SET), ==, 0);
  char *text = g_malloc0((gsize)length + 1);
  g_assert_cmpuint(fread(text, 1, (gsize)length, stream), ==, (gsize)length);
  return text;
}

static void test_helper_diagnostics_use_stdout_and_warnings_use_stderr(void) {
  FILE *captured_stdout = tmpfile();
  FILE *captured_stderr = tmpfile();
  g_assert_nonnull(captured_stdout);
  g_assert_nonnull(captured_stderr);
  int saved_stdout = dup(STDOUT_FILENO);
  int saved_stderr = dup(STDERR_FILENO);
  g_assert_cmpint(saved_stdout, >=, 0);
  g_assert_cmpint(saved_stderr, >=, 0);
  g_assert_cmpint(dup2(fileno(captured_stdout), STDOUT_FILENO), >=, 0);
  g_assert_cmpint(dup2(fileno(captured_stderr), STDERR_FILENO), >=, 0);

  diagnostic_line("portal detail");
  log_line("portal warning");
  status_notifier_diagnostic("notifier detail");
  status_notifier_log("notifier warning");
  fflush(stdout);
  fflush(stderr);

  g_assert_cmpint(dup2(saved_stdout, STDOUT_FILENO), >=, 0);
  g_assert_cmpint(dup2(saved_stderr, STDERR_FILENO), >=, 0);
  close(saved_stdout);
  close(saved_stderr);

  char *stdout_text = read_test_stream(captured_stdout);
  char *stderr_text = read_test_stream(captured_stderr);
  g_assert_nonnull(strstr(stdout_text, "portal bridge: portal detail"));
  g_assert_nonnull(
      strstr(stdout_text, "status notifier bridge: notifier detail"));
  g_assert_null(strstr(stdout_text, "warning"));
  g_assert_nonnull(strstr(stderr_text, "portal bridge: portal warning"));
  g_assert_nonnull(
      strstr(stderr_text, "status notifier bridge: notifier warning"));
  g_assert_null(strstr(stderr_text, "detail"));

  g_free(stdout_text);
  g_free(stderr_text);
  fclose(captured_stdout);
  fclose(captured_stderr);
}

static void free_test_mount_call(TestMountCall *call) {
  g_free(call->source);
  g_free(call->target);
  g_free(call);
}

bool mount_grant_path(const char *source, const char *target, bool read_only,
                      GError **error) {
  (void)error;
  TestMountCall *call = g_new0(TestMountCall, 1);
  call->source = g_strdup(source);
  call->target = g_strdup(target);
  call->read_only = read_only;
  g_ptr_array_add(test_mount_calls, call);
  return true;
}

bool unmount_path(const char *target) {
  (void)target;
  return test_unmount_succeeds;
}

static void test_flatpak_spawn_contract(void) {
  GError *error = NULL;
  GDBusNodeInfo *node = g_dbus_node_info_new_for_xml(FLATPAK_SPAWN_XML, &error);
  g_assert_no_error(error);
  GDBusInterfaceInfo *iface = g_dbus_node_info_lookup_interface(node, "org.freedesktop.portal.Flatpak");
  g_assert_nonnull(iface);
  GDBusMethodInfo *spawn = g_dbus_interface_info_lookup_method(iface, "Spawn");
  g_assert_nonnull(spawn);
  const char *expected[] = {"ay", "aay", "a{uh}", "a{ss}", "u", "a{sv}"};
  for (guint i = 0; i < 6; i++) g_assert_cmpstr(spawn->in_args[i]->signature, ==, expected[i]);
  g_assert_cmpstr(spawn->out_args[0]->signature, ==, "u");
  g_assert_cmpstr(g_dbus_interface_info_lookup_method(iface, "SpawnSignal")->in_args[1]->signature, ==, "u");
  g_assert_cmpstr(g_dbus_interface_info_lookup_signal(iface, "SpawnStarted")->args[1]->signature, ==, "u");
  g_assert_cmpstr(g_dbus_interface_info_lookup_signal(iface, "SpawnExited")->args[1]->signature, ==, "u");

  GVariant *version = FLATPAK_SPAWN_VTABLE.get_property(
      NULL, NULL, NULL, NULL, "version", &error, NULL);
  g_assert_no_error(error);
  g_assert_cmpuint(g_variant_get_uint32(version), ==, 4);
  g_variant_unref(version);
  GVariant *supports = FLATPAK_SPAWN_VTABLE.get_property(
      NULL, NULL, NULL, NULL, "supports", &error, NULL);
  g_assert_no_error(error);
  g_assert_cmpuint(g_variant_get_uint32(supports), ==, 1);
  g_variant_unref(supports);
  g_assert_true(flatpak_spawn_flags_supported(4 | 32 | 64));
  g_assert_false(flatpak_spawn_flags_supported(128));
  g_dbus_node_info_unref(node);
}

static void test_introspection(void) {
  GError *error = NULL;
  GDBusNodeInfo *desktop = g_dbus_node_info_new_for_xml(DESKTOP_XML, &error);
  g_assert_no_error(error);
  g_assert_nonnull(desktop);
  GDBusInterfaceInfo *screencast = g_dbus_node_info_lookup_interface(
      desktop, "org.freedesktop.portal.ScreenCast");
  g_assert_nonnull(screencast);
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(screencast, "CreateSession"));
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(screencast, "SelectSources"));
  g_assert_nonnull(g_dbus_interface_info_lookup_method(screencast, "Start"));
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(screencast, "OpenPipeWireRemote"));
  g_assert_nonnull(g_dbus_interface_info_lookup_property(
      screencast, "AvailableSourceTypes"));
  g_assert_nonnull(g_dbus_interface_info_lookup_property(
      screencast, "AvailableCursorModes"));

  GDBusInterfaceInfo *proxy_resolver = g_dbus_node_info_lookup_interface(
      desktop, "org.freedesktop.portal.ProxyResolver");
  g_assert_nonnull(proxy_resolver);
  GDBusMethodInfo *lookup =
      g_dbus_interface_info_lookup_method(proxy_resolver, "Lookup");
  g_assert_nonnull(lookup);
  g_assert_nonnull(lookup->in_args);
  g_assert_nonnull(lookup->in_args[0]);
  g_assert_cmpstr(lookup->in_args[0]->signature, ==, "s");
  g_assert_null(lookup->in_args[1]);
  g_assert_nonnull(lookup->out_args);
  g_assert_nonnull(lookup->out_args[0]);
  g_assert_cmpstr(lookup->out_args[0]->signature, ==, "as");
  g_assert_null(lookup->out_args[1]);
  g_dbus_node_info_unref(desktop);

  GDBusNodeInfo *session = g_dbus_node_info_new_for_xml(SESSION_XML, &error);
  g_assert_no_error(error);
  g_assert_nonnull(session);
  GDBusInterfaceInfo *session_interface = g_dbus_node_info_lookup_interface(
      session, "org.freedesktop.portal.Session");
  g_assert_nonnull(session_interface);
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(session_interface, "Close"));
  g_assert_nonnull(
      g_dbus_interface_info_lookup_signal(session_interface, "Closed"));
  g_dbus_node_info_unref(session);

  GDBusNodeInfo *control = g_dbus_node_info_new_for_xml(CONTROL_XML, &error);
  g_assert_no_error(error);
  g_assert_nonnull(control);
  GDBusInterfaceInfo *control_interface = g_dbus_node_info_lookup_interface(
      control, "org.freebsd.Flatpak.PortalBridge");
  g_assert_nonnull(control_interface);
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(control_interface, "AddSandbox"));
  g_assert_nonnull(
      g_dbus_interface_info_lookup_method(control_interface, "RemoveSandbox"));
  g_dbus_node_info_unref(control);

  GDBusNodeInfo *watcher =
      g_dbus_node_info_new_for_xml(STATUS_WATCHER_XML, &error);
  g_assert_no_error(error);
  g_assert_nonnull(watcher);
  g_assert_nonnull(g_dbus_node_info_lookup_interface(
      watcher, "org.kde.StatusNotifierWatcher"));
  g_dbus_node_info_unref(watcher);
}

static void test_shared_sandbox_scope_validation(void) {
  BridgeState state = {
      .documents.sandbox_root = "/runtime/chroots/org.example.App",
  };
  g_assert_true(sandbox_doc_dir_allowed(
      &state, "/runtime/chroots/org.example.App/one/run/user/1001/doc"));
  g_assert_true(sandbox_doc_dir_allowed(
      &state, "/runtime/chroots/org.example.App/two/run/user/1001/doc"));
  g_assert_false(sandbox_doc_dir_allowed(
      &state, "/runtime/chroots/org.example.Other/one/run/user/1001/doc"));
  g_assert_false(sandbox_doc_dir_allowed(
      &state, "/runtime/chroots/org.example.App/../org.example.Other/doc"));
}

static void test_remove_sandbox_is_per_instance_and_idempotent(void) {
  BridgeState state = {0};
  state.documents.grants = g_ptr_array_new();
  state.documents.sandbox_doc_dirs = g_ptr_array_new_with_free_func(g_free);
  const char *first =
      "/runtime/chroots/org.example.App/first/run/user/1001/doc";
  const char *second =
      "/runtime/chroots/org.example.App/second/run/user/1001/doc";
  g_ptr_array_add(state.documents.sandbox_doc_dirs, g_strdup(first));
  g_ptr_array_add(state.documents.sandbox_doc_dirs, g_strdup(second));

  g_assert_true(remove_sandbox(&state, first, NULL));
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 1);
  g_assert_cmpstr(g_ptr_array_index(state.documents.sandbox_doc_dirs, 0), ==,
                  second);

  g_assert_true(remove_sandbox(&state, first, NULL));
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 1);
  g_assert_true(remove_sandbox(&state, second, NULL));
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 0);

  g_ptr_array_free(state.documents.sandbox_doc_dirs, TRUE);
  g_ptr_array_free(state.documents.grants, TRUE);
}

static void test_failed_document_unmount_keeps_the_document_root_registered(void) {
  BridgeState state = {0};
  const char *doc_dir =
      "/runtime/chroots/org.example.App/first/run/user/1001/doc";
  DocumentGrant *grant = g_new0(DocumentGrant, 1);
  grant->target_paths = g_ptr_array_new_with_free_func(g_free);
  g_ptr_array_add(grant->target_paths,
                  g_strdup("/runtime/chroots/org.example.App/first/run/user/1001/doc/grant/file"));
  state.documents.grants =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_grant);
  state.documents.sandbox_doc_dirs = g_ptr_array_new_with_free_func(g_free);
  g_ptr_array_add(state.documents.grants, grant);
  g_ptr_array_add(state.documents.sandbox_doc_dirs, g_strdup(doc_dir));

  test_unmount_succeeds = false;
  GError *error = NULL;
  g_assert_false(remove_sandbox(&state, doc_dir, &error));
  g_assert_error(error, G_IO_ERROR, G_IO_ERROR_BUSY);
  g_clear_error(&error);
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 1);
  g_assert_cmpuint(grant->target_paths->len, ==, 1);

  test_unmount_succeeds = true;
  g_assert_true(remove_sandbox(&state, doc_dir, NULL));
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 0);
  g_assert_cmpuint(grant->target_paths->len, ==, 0);
  g_ptr_array_free(state.documents.grants, TRUE);
  g_ptr_array_free(state.documents.sandbox_doc_dirs, TRUE);
}

static void test_path_and_option_translation(void) {
  char *path = portal_path("request", ":1.42", "chromium.request-7");
  g_assert_cmpstr(
      path, ==,
      "/org/freedesktop/portal/desktop/request/1_42/chromium_request_7");
  g_free(path);

  GVariantBuilder builder;
  g_variant_builder_init(&builder, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&builder, "{sv}", "handle_token",
                        g_variant_new_string("local-request"));
  g_variant_builder_add(&builder, "{sv}", "session_handle_token",
                        g_variant_new_string("local-session"));
  g_variant_builder_add(&builder, "{sv}", "types", g_variant_new_uint32(3));
  g_variant_builder_add(&builder, "{sv}", "multiple",
                        g_variant_new_boolean(TRUE));
  GVariant *options = g_variant_ref_sink(g_variant_builder_end(&builder));
  GVariant *rewritten = g_variant_ref_sink(
      rewrite_options(options, "host-request", "host-session"));

  const char *handle_token = NULL;
  const char *session_token = NULL;
  guint32 types = 0;
  gboolean multiple = FALSE;
  g_assert_true(
      g_variant_lookup(rewritten, "handle_token", "&s", &handle_token));
  g_assert_true(g_variant_lookup(rewritten, "session_handle_token", "&s",
                                 &session_token));
  g_assert_true(g_variant_lookup(rewritten, "types", "u", &types));
  g_assert_true(g_variant_lookup(rewritten, "multiple", "b", &multiple));
  g_assert_cmpstr(handle_token, ==, "host-request");
  g_assert_cmpstr(session_token, ==, "host-session");
  g_assert_cmpuint(types, ==, 3);
  g_assert_true(multiple);
  g_variant_unref(rewritten);
  g_variant_unref(options);
}

static void test_unix_fd_copy(void) {
  int pipe_fds[2];
  g_assert_cmpint(pipe(pipe_fds), ==, 0);
  GError *error = NULL;
  GUnixFDList *host_fds = g_unix_fd_list_new();
  gint32 host_index = g_unix_fd_list_append(host_fds, pipe_fds[0], &error);
  g_assert_no_error(error);
  g_assert_cmpint(host_index, >=, 0);
  close(pipe_fds[0]);

  GUnixFDList *local_fds = g_unix_fd_list_new();
  gint32 local_index = copy_unix_fd(host_fds, host_index, local_fds, &error);
  g_assert_no_error(error);
  g_assert_cmpint(local_index, >=, 0);
  int local_fd = g_unix_fd_list_get(local_fds, local_index, &error);
  g_assert_no_error(error);
  g_assert_cmpint(local_fd, >=, 0);

  const char byte = 'P';
  g_assert_cmpint(write(pipe_fds[1], &byte, 1), ==, 1);
  char received = '\0';
  g_assert_cmpint(read(local_fd, &received, 1), ==, 1);
  g_assert_cmpint(received, ==, byte);

  close(local_fd);
  close(pipe_fds[1]);
  g_object_unref(local_fds);
  g_object_unref(host_fds);
}

static void cleanup_document_test_state(BridgeState *state) {
  for (guint i = 0; i < state->documents.grants->len; i++) {
    cleanup_grant(g_ptr_array_index(state->documents.grants, i));
  }
  g_ptr_array_free(state->documents.grants, TRUE);
  g_ptr_array_free(state->documents.sandbox_doc_dirs, TRUE);
}

static void init_document_test_state(BridgeState *state, const char *root,
                                     const char *store) {
  memset(state, 0, sizeof(*state));
  state->app_id = "org.example.App";
  state->documents.doc_dir = g_build_filename(root, "doc", NULL);
  state->documents.mountpoint = "/run/user/1001/doc";
  state->documents.persistent_store = (char *)store;
  state->documents.grants =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_grant);
  state->documents.sandbox_doc_dirs =
      g_ptr_array_new_with_free_func(g_free);
  g_assert_cmpint(g_mkdir_with_parents(state->documents.doc_dir, 0700), ==, 0);
}

static void assert_direct_grant_mount(TestMountCall *call,
                                      const char *host_dir,
                                      const char *sandbox_doc_dir,
                                      const char *doc_id) {
  char *base = g_path_get_basename(host_dir);
  char *expected_target =
      g_build_filename(sandbox_doc_dir, doc_id, base, NULL);
  g_assert_cmpstr(call->source, ==, host_dir);
  g_assert_cmpstr(call->target, ==, expected_target);
  g_assert_false(call->read_only);
  g_free(expected_target);
  g_free(base);
}

static void remove_sandbox_test_path(const char *sandbox_doc_dir) {
  char *uid_dir = g_path_get_dirname(sandbox_doc_dir);
  char *user_dir = g_path_get_dirname(uid_dir);
  char *run_dir = g_path_get_dirname(user_dir);
  char *instance_dir = g_path_get_dirname(run_dir);
  g_assert_cmpint(g_rmdir(sandbox_doc_dir), ==, 0);
  g_assert_cmpint(g_rmdir(uid_dir), ==, 0);
  g_assert_cmpint(g_rmdir(user_dir), ==, 0);
  g_assert_cmpint(g_rmdir(run_dir), ==, 0);
  g_assert_cmpint(g_rmdir(instance_dir), ==, 0);
  g_free(instance_dir);
  g_free(run_dir);
  g_free(user_dir);
  g_free(uid_dir);
}

static void test_grant_mount_order_is_direct_and_equivalent(void) {
  GError *error = NULL;
  char *root = g_dir_make_tmp("freebsd-flatpak-order-test-XXXXXX", &error);
  g_assert_no_error(error);
  char *host_dir = g_build_filename(root, "Selected", NULL);
  char *store = g_build_filename(root, "grants.ini", NULL);
  char *sandbox_root = g_build_filename(root, "sandboxes", NULL);
  char *first_sandbox = g_build_filename(
      sandbox_root, "first", "run", "user", "1001", "doc", NULL);
  char *second_sandbox = g_build_filename(
      sandbox_root, "second", "run", "user", "1001", "doc", NULL);
  g_assert_cmpint(g_mkdir(host_dir, 0700), ==, 0);
  g_assert_cmpint(g_mkdir_with_parents(first_sandbox, 0700), ==, 0);
  g_assert_cmpint(g_mkdir_with_parents(second_sandbox, 0700), ==, 0);

  test_mount_calls =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_test_mount_call);

  BridgeState grant_first;
  init_document_test_state(&grant_first, root, store);
  grant_first.documents.sandbox_root = sandbox_root;
  char **permissions = read_write_permissions();
  DocumentGrant *first_grant = NULL;
  g_assert_true(create_document_grant_from_path(
      &grant_first, host_dir, grant_first.app_id, permissions, true, false,
      false,
      &first_grant, &error));
  g_assert_no_error(error);
  g_assert_true(register_document_grant(&grant_first, first_grant, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(test_mount_calls->len, ==, 0);
  g_assert_true(add_sandbox(&grant_first, first_sandbox, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(test_mount_calls->len, ==, 1);
  assert_direct_grant_mount(g_ptr_array_index(test_mount_calls, 0), host_dir,
                            first_sandbox, first_grant->doc_id);
  cleanup_document_test_state(&grant_first);
  g_free(grant_first.documents.doc_dir);
  g_ptr_array_set_size(test_mount_calls, 0);

  BridgeState sandbox_first;
  init_document_test_state(&sandbox_first, root, store);
  sandbox_first.documents.sandbox_root = sandbox_root;
  g_assert_true(add_sandbox(&sandbox_first, second_sandbox, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(test_mount_calls->len, ==, 0);
  DocumentGrant *second_grant = NULL;
  g_assert_true(create_document_grant_from_path(
      &sandbox_first, host_dir, sandbox_first.app_id, permissions, true, false,
      false,
      &second_grant, &error));
  g_assert_no_error(error);
  g_assert_true(register_document_grant(&sandbox_first, second_grant, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(test_mount_calls->len, ==, 1);
  assert_direct_grant_mount(g_ptr_array_index(test_mount_calls, 0), host_dir,
                            second_sandbox, second_grant->doc_id);
  cleanup_document_test_state(&sandbox_first);
  g_free(sandbox_first.documents.doc_dir);

  g_strfreev(permissions);
  g_ptr_array_free(test_mount_calls, TRUE);
  test_mount_calls = NULL;
  remove_sandbox_test_path(first_sandbox);
  remove_sandbox_test_path(second_sandbox);
  g_assert_cmpint(g_rmdir(sandbox_root), ==, 0);
  g_assert_cmpint(g_rmdir(host_dir), ==, 0);
  char *doc_dir = g_build_filename(root, "doc", NULL);
  g_assert_cmpint(g_rmdir(doc_dir), ==, 0);
  g_assert_cmpint(g_rmdir(root), ==, 0);
  g_free(doc_dir);
  g_free(second_sandbox);
  g_free(first_sandbox);
  g_free(sandbox_root);
  g_free(store);
  g_free(host_dir);
  g_free(root);
}

static void test_directory_grant_persists_and_translates(void) {
  GError *error = NULL;
  char *root = g_dir_make_tmp("freebsd-flatpak-doc-test-XXXXXX", &error);
  g_assert_no_error(error);
  char *host_dir = g_build_filename(root, "Selected Folder", NULL);
  char *store = g_build_filename(root, "grants.ini", NULL);
  g_assert_cmpint(g_mkdir(host_dir, 0700), ==, 0);

  BridgeState first;
  init_document_test_state(&first, root, store);
  char **permissions = read_write_permissions();
  DocumentGrant *grant = NULL;
  g_assert_true(create_document_grant_from_path(
      &first, host_dir, first.app_id, permissions, true, true, false, &grant,
      &error));
  g_assert_no_error(error);
  g_assert_true(register_document_grant(&first, grant, &error));
  g_assert_no_error(error);
  g_assert_true(grant->is_directory);
  g_assert_true(grant->persistent);
  g_assert_cmpuint(strlen(grant->doc_id), ==, 22);
  g_assert_true(document_id_is_valid(grant->doc_id));
  g_assert_true(
      g_strv_contains((const char *const *)grant->permissions, "write"));
  char *doc_id = g_strdup(grant->doc_id);
  DocumentGrant *reused = NULL;
  g_assert_true(create_document_grant_from_path(
      &first, host_dir, first.app_id, permissions, true, true, true, &reused,
      &error));
  g_assert_no_error(error);
  g_assert_true(reused == grant);
  g_assert_true(register_document_grant(&first, reused, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(first.documents.grants->len, ==, 1);
  char *uri = sandbox_uri_for_grant(&first, grant);
  g_assert_true(g_str_has_prefix(uri, "file:///run/user/1001/doc/"));
  g_assert_nonnull(strstr(uri, "/Selected%20Folder"));
  g_assert_true(g_file_test(store, G_FILE_TEST_IS_REGULAR));
  g_free(uri);
  g_strfreev(permissions);
  cleanup_document_test_state(&first);
  g_free(first.documents.doc_dir);

  BridgeState second;
  init_document_test_state(&second, root, store);
  g_assert_true(load_persistent_document_grants(&second, &error));
  g_assert_no_error(error);
  g_assert_cmpuint(second.documents.grants->len, ==, 1);
  DocumentGrant *restored = find_grant(&second, doc_id);
  g_assert_nonnull(restored);
  g_assert_cmpstr(restored->host_path, ==, host_dir);
  g_assert_true(restored->is_directory);
  g_assert_true(
      g_strv_contains((const char *const *)restored->permissions, "write"));

  char *base = g_path_get_basename(host_dir);
  char *document_path = g_build_filename(second.documents.mountpoint, doc_id,
                                         base, "nested", NULL);
  char *translated = host_path_for_document_path(&second, document_path);
  char *expected = g_build_filename(host_dir, "nested", NULL);
  g_assert_cmpstr(translated, ==, expected);
  char *traversal = g_strconcat(document_path, "/../outside", NULL);
  g_assert_null(host_path_for_document_path(&second, traversal));

  GVariantBuilder options;
  g_variant_builder_init(&options, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&options, "{sv}", "current_folder",
                        g_variant_new_bytestring(document_path));
  GVariant *parameters = g_variant_ref_sink(g_variant_new(
      "(ss@a{sv})", "", "Choose", g_variant_builder_end(&options)));
  GVariant *rewritten =
      g_variant_ref_sink(rewrite_filechooser_parameters(&second, parameters));
  GVariant *rewritten_options = g_variant_get_child_value(rewritten, 2);
  const char *rewritten_folder = NULL;
  g_assert_true(g_variant_lookup(rewritten_options, "current_folder", "^&ay",
                                 &rewritten_folder));
  g_assert_cmpstr(rewritten_folder, ==, expected);

  g_variant_unref(rewritten_options);
  g_variant_unref(rewritten);
  g_variant_unref(parameters);
  g_free(traversal);
  g_free(expected);
  g_free(translated);
  g_free(document_path);
  g_free(base);
  cleanup_document_test_state(&second);
  g_free(second.documents.doc_dir);
  g_free(doc_id);
  g_assert_cmpint(g_remove(store), ==, 0);
  g_assert_cmpint(g_rmdir(host_dir), ==, 0);
  char *doc_dir = g_build_filename(root, "doc", NULL);
  g_assert_cmpint(g_rmdir(doc_dir), ==, 0);
  g_assert_cmpint(g_rmdir(root), ==, 0);
  g_free(doc_dir);
  g_free(store);
  g_free(host_dir);
  g_free(root);
}

static void test_ungrantable_filechooser_uri_is_not_leaked(void) {
  GError *error = NULL;
  char *root = g_dir_make_tmp("freebsd-flatpak-uri-test-XXXXXX", &error);
  g_assert_no_error(error);
  char *host_dir = g_build_filename(root, "directory", NULL);
  char *store = g_build_filename(root, "grants.ini", NULL);
  g_assert_cmpint(g_mkdir(host_dir, 0700), ==, 0);
  BridgeState state;
  init_document_test_state(&state, root, store);

  char *host_uri = g_filename_to_uri(host_dir, NULL, &error);
  g_assert_no_error(error);
  GVariantBuilder uris;
  g_variant_builder_init(&uris, G_VARIANT_TYPE_STRING_ARRAY);
  g_variant_builder_add(&uris, "s", host_uri);
  GVariantBuilder results;
  g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&results, "{sv}", "uris",
                        g_variant_builder_end(&uris));
  GVariant *result = g_variant_ref_sink(g_variant_builder_end(&results));
  GVariant *rewritten =
      g_variant_ref_sink(rewrite_filechooser_results(&state, 0, result, false));
  GVariant *rewritten_uris =
      g_variant_lookup_value(rewritten, "uris", G_VARIANT_TYPE("as"));
  g_assert_nonnull(rewritten_uris);
  g_assert_cmpuint(g_variant_n_children(rewritten_uris), ==, 0);
  g_assert_cmpuint(state.documents.grants->len, ==, 0);

  g_variant_unref(rewritten_uris);
  g_variant_unref(rewritten);
  g_variant_unref(result);
  g_free(host_uri);
  cleanup_document_test_state(&state);
  g_free(state.documents.doc_dir);
  g_assert_cmpint(g_rmdir(host_dir), ==, 0);
  char *doc_dir = g_build_filename(root, "doc", NULL);
  g_assert_cmpint(g_rmdir(doc_dir), ==, 0);
  g_assert_cmpint(g_rmdir(root), ==, 0);
  g_free(doc_dir);
  g_free(store);
  g_free(host_dir);
  g_free(root);
}

static void test_screencast_source_tracking(void) {
  BridgeState state = {0};
  SessionRecord session = {
      .state = &state,
      .local_path = "/test/session",
      .sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource)),
  };
  GVariantBuilder stream_properties;
  g_variant_builder_init(&stream_properties, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&stream_properties, "{sv}", "pipewire-serial",
                        g_variant_new_uint64(1234));
  GVariantBuilder streams;
  g_variant_builder_init(&streams, G_VARIANT_TYPE("a(ua{sv})"));
  g_variant_builder_add(&streams, "(u@a{sv})", 45,
                        g_variant_builder_end(&stream_properties));
  GVariantBuilder results;
  g_variant_builder_init(&results, G_VARIANT_TYPE_VARDICT);
  g_variant_builder_add(&results, "{sv}", "streams",
                        g_variant_builder_end(&streams));
  GVariant *result = g_variant_ref_sink(g_variant_builder_end(&results));

  update_session_sources(&session, result);
  g_assert_cmpuint(session.sources->len, ==, 1);
  ScreenCastSource *source =
      &g_array_index(session.sources, ScreenCastSource, 0);
  g_assert_cmpuint(source->node_id, ==, 45);
  g_assert_cmpuint(source->serial, ==, 1234);

  g_variant_unref(result);
  g_array_free(session.sources, TRUE);
}

static void test_pipewire_client_session_ownership(void) {
  BridgeState state = {0};
  state.screencast.sessions = g_ptr_array_new();
  SessionRecord session = {
      .state = &state,
      .sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource)),
  };
  ScreenCastSource approved = {.node_id = 45, .serial = 52};
  g_array_append_val(session.sources, approved);
  g_ptr_array_add(state.screencast.sessions, &session);

  PipeWireClient client = {
      .permissions = g_array_new(FALSE, TRUE, sizeof(struct pw_permission)),
      .is_portal = true,
      .permissions_received = true,
  };
  struct pw_permission deny_other = {
      .id = PW_ID_ANY,
      .permissions = 0,
  };
  struct pw_permission allow_approved = {
      .id = 45,
      .permissions = PW_PERM_RWX,
  };
  g_array_append_val(client.permissions, deny_other);
  g_array_append_val(client.permissions, allow_approved);
  g_assert_true(pipewire_client_matches_session(&client, &session));

  SessionRecord other_session = {
      .state = &state,
      .sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource)),
  };
  ScreenCastSource other = {.node_id = 99, .serial = 100};
  g_array_append_val(other_session.sources, other);
  g_ptr_array_add(state.screencast.sessions, &other_session);
  struct pw_permission allow_other = {
      .id = 99,
      .permissions = PW_PERM_R,
  };
  g_array_append_val(client.permissions, allow_other);
  g_assert_false(pipewire_client_matches_session(&client, &session));

  client.is_portal = false;
  g_assert_false(pipewire_client_matches_session(&client, &session));

  g_array_free(client.permissions, TRUE);
  g_array_free(other_session.sources, TRUE);
  g_array_free(session.sources, TRUE);
  g_ptr_array_free(state.screencast.sessions, TRUE);
}

static void test_pipewire_source_generation_tracking(void) {
  SessionRecord session = {
      .sources = g_array_new(FALSE, TRUE, sizeof(ScreenCastSource)),
  };
  ScreenCastSource current = {.node_id = 47, .serial = 149};
  g_array_append_val(session.sources, current);

  PipeWireNode stale = {.id = 47, .serial = 141};
  g_assert_false(source_node_is_approved(&session, &stale));
  remove_session_source_for_node(&session, &stale);
  g_assert_cmpuint(session.sources->len, ==, 1);

  PipeWireNode matching = {.id = 47, .serial = 149};
  g_assert_true(source_node_is_approved(&session, &matching));
  remove_session_source_for_node(&session, &matching);
  g_assert_cmpuint(session.sources->len, ==, 0);

  g_array_free(session.sources, TRUE);
}

static void save_test_icon(const char *path) {
  GError *error = NULL;
  GdkPixbuf *pixbuf =
      gdk_pixbuf_new(GDK_COLORSPACE_RGB, TRUE, 8, 2, 1);
  g_assert_nonnull(pixbuf);
  guchar *pixels = gdk_pixbuf_get_pixels(pixbuf);
  pixels[0] = 0x11;
  pixels[1] = 0x22;
  pixels[2] = 0x33;
  pixels[3] = 0x44;
  pixels[4] = 0xaa;
  pixels[5] = 0xbb;
  pixels[6] = 0xcc;
  pixels[7] = 0xdd;
  g_assert_true(gdk_pixbuf_save(pixbuf, path, "png", &error, NULL));
  g_assert_no_error(error);
  g_object_unref(pixbuf);
}

static void assert_test_icon_pixmap(GVariant *pixmaps) {
  g_assert_nonnull(pixmaps);
  g_assert_cmpuint(g_variant_n_children(pixmaps), ==, 1);
  GVariant *entry = g_variant_get_child_value(pixmaps, 0);
  int width = 0;
  int height = 0;
  GVariant *bytes = NULL;
  g_variant_get(entry, "(ii@ay)", &width, &height, &bytes);
  g_assert_cmpint(width, ==, 2);
  g_assert_cmpint(height, ==, 1);
  gsize size = 0;
  const guchar *argb = g_variant_get_fixed_array(bytes, &size, 1);
  const guchar expected[] = {0x44, 0x11, 0x22, 0x33,
                             0xdd, 0xaa, 0xbb, 0xcc};
  g_assert_cmpuint(size, ==, sizeof(expected));
  g_assert_cmpmem(argb, size, expected, sizeof(expected));
  g_variant_unref(bytes);
  g_variant_unref(entry);
  g_variant_unref(pixmaps);
}

static void test_status_icon_resolution(void) {
  GError *error = NULL;
  char *root = g_dir_make_tmp("freebsd-flatpak-icon-test-XXXXXX", &error);
  g_assert_no_error(error);
  char *app_root = g_build_filename(root, "app", NULL);
  char *runtime_root = g_build_filename(root, "runtime", NULL);
  char *icon_dir = g_build_filename(app_root, "share", "icons", "hicolor",
                                    "32x32", "apps", NULL);
  char *named_icon = g_build_filename(icon_dir, "org.example.Tray.png", NULL);
  char *runtime_icon_dir =
      g_build_filename(runtime_root, "custom", NULL);
  char *runtime_icon = g_build_filename(runtime_icon_dir, "absolute.png", NULL);
  g_assert_cmpint(g_mkdir_with_parents(icon_dir, 0700), ==, 0);
  g_assert_cmpint(g_mkdir_with_parents(runtime_icon_dir, 0700), ==, 0);
  save_test_icon(named_icon);
  save_test_icon(runtime_icon);

  StatusNotifierBridge state = {
      .app_root = app_root,
      .runtime_root = runtime_root,
  };
  assert_test_icon_pixmap(
      resolve_status_icon(&state, "org.example.Tray", ""));
  assert_test_icon_pixmap(
      resolve_status_icon(&state, "/usr/custom/absolute.png", ""));
  g_assert_null(resolve_status_icon(&state, "org.example.Missing", ""));
  g_assert_null(resolve_status_icon(&state, "../outside", ""));
  g_assert_null(
      resolve_status_icon(&state, "/app/../../runtime/custom/absolute.png", ""));

  g_assert_cmpint(g_remove(runtime_icon), ==, 0);
  g_assert_cmpint(g_remove(named_icon), ==, 0);
  char *directory = g_strdup(icon_dir);
  while (g_strcmp0(directory, app_root) != 0) {
    g_assert_cmpint(g_rmdir(directory), ==, 0);
    char *parent = g_path_get_dirname(directory);
    g_free(directory);
    directory = parent;
  }
  g_assert_cmpint(g_rmdir(app_root), ==, 0);
  g_assert_cmpint(g_rmdir(runtime_icon_dir), ==, 0);
  g_assert_cmpint(g_rmdir(runtime_root), ==, 0);
  g_assert_cmpint(g_rmdir(root), ==, 0);
  g_free(directory);
  g_free(runtime_icon);
  g_free(runtime_icon_dir);
  g_free(named_icon);
  g_free(icon_dir);
  g_free(runtime_root);
  g_free(app_root);
  g_free(root);
}

static void test_flatpak_lifecycle_source_is_async(void) {
  int pair[2];
  g_assert_cmpint(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, pair), ==, 0);
  BridgeState state = { .spawn_lifecycles = g_ptr_array_new_with_free_func((GDestroyNotify)flatpak_spawn_lifecycle_free) };
  flatpak_spawn_watch_lifecycle(&state, pair[1], 55, 55, NULL, 0);
  unsigned char accepted[24] = {0}; guint32 magic = htonl(0x46534250), request = htonl(55), length = htonl(4); guint16 version = htons(1), type = htons(6);
  memcpy(accepted, &magic, 4); memcpy(accepted + 4, &version, 2); memcpy(accepted + 6, &type, 2); memcpy(accepted + 8, &request, 4); memcpy(accepted + 12, &length, 4); memcpy(accepted + 20, &request, 4);
  g_assert_cmpint(send(pair[0], accepted, sizeof(accepted), 0), ==, sizeof(accepted));
  g_main_context_iteration(NULL, TRUE);
  g_assert_cmpuint(state.spawn_lifecycles->len, ==, 1);
  unsigned char started[28] = {0}; guint32 pid = htonl(55); type = htons(12); length = htonl(8);
  memcpy(started, &magic, 4); memcpy(started + 4, &version, 2); memcpy(started + 6, &type, 2); memcpy(started + 8, &request, 4); memcpy(started + 12, &length, 4); memcpy(started + 20, &pid, 4); memcpy(started + 24, &pid, 4);
  g_assert_cmpint(send(pair[0], started, sizeof(started), 0), ==, sizeof(started));
  g_main_context_iteration(NULL, TRUE);
  g_assert_cmpuint(state.spawn_lifecycles->len, ==, 1);
  unsigned char exited[28] = {0}; type = htons(7); length = htonl(8);
  memcpy(exited, &magic, 4); memcpy(exited + 4, &version, 2); memcpy(exited + 6, &type, 2); memcpy(exited + 8, &request, 4); memcpy(exited + 12, &length, 4); memcpy(exited + 20, &request, 4);
  g_assert_cmpint(send(pair[0], exited, sizeof(exited), 0), ==, sizeof(exited));
  g_main_context_iteration(NULL, TRUE);
  g_assert_cmpuint(state.spawn_lifecycles->len, ==, 0);
  close(pair[0]);
  g_ptr_array_free(state.spawn_lifecycles, TRUE);
}

static void test_flatpak_watch_bus_lifecycle_closes_only_matching_spawns(void) {
  int watched[2], ordinary[2], other_sender[2];
  g_assert_cmpint(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, watched), ==, 0);
  g_assert_cmpint(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, ordinary), ==, 0);
  g_assert_cmpint(socketpair(AF_UNIX, SOCK_SEQPACKET, 0, other_sender), ==, 0);
  BridgeState state = { .spawn_lifecycles = g_ptr_array_new_with_free_func((GDestroyNotify)flatpak_spawn_lifecycle_free) };
  flatpak_spawn_watch_lifecycle(&state, watched[1], 1, 101, ":1.5", 16);
  flatpak_spawn_watch_lifecycle(&state, ordinary[1], 2, 102, ":1.5", 0);
  flatpak_spawn_watch_lifecycle(&state, other_sender[1], 3, 103, ":1.6", 16);

  unsigned char exited[28] = {0}; guint32 magic = htonl(0x46534250), exited_request = htonl(1), length = htonl(8), exited_pid = htonl(101); guint16 version = htons(1), exited_type = htons(10);
  memcpy(exited, &magic, 4); memcpy(exited + 4, &version, 2); memcpy(exited + 6, &exited_type, 2); memcpy(exited + 8, &exited_request, 4); memcpy(exited + 12, &length, 4); memcpy(exited + 20, &exited_pid, 4);
  g_assert_cmpint(send(watched[0], exited, sizeof(exited), 0), ==, sizeof(exited));
  while (g_main_context_iteration(NULL, FALSE));
  g_assert_cmpuint(state.spawn_lifecycles->len, ==, 3);

  flatpak_spawn_close_watch_bus_lifecycles(&state, ":1.5");

  g_assert_cmpuint(state.spawn_lifecycles->len, ==, 2);
  unsigned char terminate[20]; guint16 type; guint32 request;
  g_assert_cmpint(recv(watched[0], terminate, sizeof(terminate), MSG_DONTWAIT), ==, sizeof(terminate));
  memcpy(&type, terminate + 6, sizeof(type));
  memcpy(&request, terminate + 8, sizeof(request));
  g_assert_cmpuint(ntohs(type), ==, 11);
  g_assert_cmpuint(ntohl(request), ==, 1);
  char byte;
  g_assert_cmpint(recv(ordinary[0], &byte, sizeof(byte), MSG_DONTWAIT), ==, -1);
  g_assert_true(errno == EAGAIN || errno == EWOULDBLOCK);
  g_assert_cmpint(recv(other_sender[0], &byte, sizeof(byte), MSG_DONTWAIT), ==, -1);
  g_assert_true(errno == EAGAIN || errno == EWOULDBLOCK);

  flatpak_spawn_cleanup_lifecycles(&state);
  close(watched[0]);
  close(ordinary[0]);
  close(other_sender[0]);
  g_ptr_array_free(state.spawn_lifecycles, TRUE);
}

int main(void) {
  test_helper_diagnostics_use_stdout_and_warnings_use_stderr();
  test_flatpak_spawn_contract();
  test_flatpak_lifecycle_source_is_async();
  test_flatpak_watch_bus_lifecycle_closes_only_matching_spawns();
  test_introspection();
  test_shared_sandbox_scope_validation();
  test_remove_sandbox_is_per_instance_and_idempotent();
  test_failed_document_unmount_keeps_the_document_root_registered();
  test_path_and_option_translation();
  test_unix_fd_copy();
  test_grant_mount_order_is_direct_and_equivalent();
  test_directory_grant_persists_and_translates();
  test_ungrantable_filechooser_uri_is_not_leaked();
  test_screencast_source_tracking();
  test_pipewire_client_session_ownership();
  test_pipewire_source_generation_tracking();
  test_status_icon_resolution();
  return 0;
}
