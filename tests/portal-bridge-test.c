#include "../compatibility_helpers/portal_bridge/basic_desktop_portals.h"
#include "../compatibility_helpers/portal_bridge/document_grant_store.h"
#include "../compatibility_helpers/portal_bridge/document_portal.h"
#include "../compatibility_helpers/portal_bridge/pipewire_screencast_linker.h"
#include "../compatibility_helpers/portal_bridge/portal_request.h"
#include "../compatibility_helpers/portal_bridge/sandbox_document_registration.h"
#include "../compatibility_helpers/portal_bridge/screencast_portal.h"
#include "../compatibility_helpers/status_notifier_bridge/status_notifier_watcher.h"

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

  remove_sandbox(&state, first);
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 1);
  g_assert_cmpstr(g_ptr_array_index(state.documents.sandbox_doc_dirs, 0), ==,
                  second);

  remove_sandbox(&state, first);
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 1);
  remove_sandbox(&state, second);
  g_assert_cmpuint(state.documents.sandbox_doc_dirs->len, ==, 0);

  g_ptr_array_free(state.documents.sandbox_doc_dirs, TRUE);
  g_ptr_array_free(state.documents.grants, TRUE);
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

int main(void) {
  test_introspection();
  test_shared_sandbox_scope_validation();
  test_remove_sandbox_is_per_instance_and_idempotent();
  test_path_and_option_translation();
  test_unix_fd_copy();
  test_screencast_source_tracking();
  test_pipewire_client_session_ownership();
  test_pipewire_source_generation_tracking();
  return 0;
}
