#include "document_portal.h"
#include "basic_desktop_portals.h"
#include "document_grant_store.h"
#include "document_grant_persistence.h"
#include "document_mounts.h"
#include "portal_bridge_process.h"
const char *DOCUMENTS_XML =
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

void handle_add_full(BridgeState *state, GDBusMethodInvocation *invocation) {
  GVariant *parameters = g_dbus_method_invocation_get_parameters(invocation);
  GVariant *handles = g_variant_get_child_value(parameters, 0);
  GVariant *permissions = g_variant_get_child_value(parameters, 3);
  const char *app_id = NULL;
  g_variant_get_child(parameters, 2, "&s", &app_id);
  guint32 flags = 0;
  g_variant_get_child(parameters, 1, "u", &flags);
  bool expected_directory = (flags & 8u) != 0;
  bool persistent = (flags & 2u) != 0;
  bool reuse_existing = (flags & 1u) != 0;

  GDBusMessage *message = g_dbus_method_invocation_get_message(invocation);
  GUnixFDList *fd_list = g_dbus_message_get_unix_fd_list(message);
  if (fd_list == NULL) {
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_INVALID_ARGUMENT,
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
    if (!create_document_grant_from_fd(state, fd, app_id, permissions,
                                       expected_directory, persistent,
                                       reuse_existing, &grant, &error)) {
      close(fd);
      g_dbus_method_invocation_take_error(invocation, error);
      g_variant_unref(handles);
      g_variant_unref(permissions);
      return;
    }
    close(fd);
    if (!register_document_grant(state, grant, &error)) {
      g_dbus_method_invocation_take_error(invocation, error);
      g_variant_unref(handles);
      g_variant_unref(permissions);
      return;
    }
    g_variant_builder_add(&ids, "s", grant->doc_id);
  }

  GVariantBuilder extra;
  g_variant_builder_init(&extra, G_VARIANT_TYPE("a{sv}"));
  add_mountpoint_extra(state, &extra);
  g_dbus_method_invocation_return_value(
      invocation,
      g_variant_new("(as@a{sv})", &ids, g_variant_builder_end(&extra)));
  g_variant_unref(handles);
  g_variant_unref(permissions);
}

void handle_delete(BridgeState *state, GDBusMethodInvocation *invocation) {
  const char *doc_id = NULL;
  g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)",
                &doc_id);
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    if (g_strcmp0(grant->doc_id, doc_id) == 0) {
      cleanup_grant(grant);
      g_ptr_array_remove_index(state->documents.grants, i);
      GError *error = NULL;
      if (!save_persistent_document_grants(state, &error)) {
        g_dbus_method_invocation_take_error(invocation, error);
        return;
      }
      g_dbus_method_invocation_return_value(invocation, NULL);
      return;
    }
  }
  g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                        G_IO_ERROR_NOT_FOUND,
                                        "No such document: %s", doc_id);
}

void return_lookup(BridgeState *state, GDBusMethodInvocation *invocation) {
  GVariant *path_variant = g_variant_get_child_value(
      g_dbus_method_invocation_get_parameters(invocation), 0);
  gsize size = 0;
  const gchar *path =
      g_variant_get_fixed_array(path_variant, &size, sizeof(guchar));
  const char *doc_id = "";
  if (path != NULL) {
    for (guint i = 0; i < state->documents.grants->len; i++) {
      DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
      if (g_strcmp0(grant->host_path, path) == 0) {
        doc_id = grant->doc_id;
        break;
      }
    }
  }
  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(s)", doc_id));
  g_variant_unref(path_variant);
}

void return_info(BridgeState *state, GDBusMethodInvocation *invocation) {
  const char *doc_id = NULL;
  g_variant_get(g_dbus_method_invocation_get_parameters(invocation), "(&s)",
                &doc_id);
  DocumentGrant *grant = find_grant(state, doc_id);
  if (grant == NULL) {
    g_dbus_method_invocation_return_error(invocation, G_IO_ERROR,
                                          G_IO_ERROR_NOT_FOUND,
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
  g_variant_builder_add(&apps, "{s@as}", grant->app_id,
                        g_variant_builder_end(&permissions));
  g_dbus_method_invocation_return_value(
      invocation, g_variant_new("(@aya{sas})",
                                path_bytes_variant(grant->host_path), &apps));
}

void return_list(BridgeState *state, GDBusMethodInvocation *invocation) {
  GVariantBuilder docs;
  g_variant_builder_init(&docs, G_VARIANT_TYPE("a{say}"));
  for (guint i = 0; i < state->documents.grants->len; i++) {
    DocumentGrant *grant = g_ptr_array_index(state->documents.grants, i);
    g_variant_builder_add(&docs, "{s@ay}", grant->doc_id,
                          path_bytes_variant(grant->host_path));
  }
  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(a{say})", &docs));
}

void return_host_paths(BridgeState *state, GDBusMethodInvocation *invocation) {
  GVariant *doc_ids = g_variant_get_child_value(
      g_dbus_method_invocation_get_parameters(invocation), 0);
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

  g_dbus_method_invocation_return_value(invocation,
                                        g_variant_new("(a{say})", &paths));
  g_variant_unref(doc_ids);
}

void handle_documents_method(GDBusConnection *connection, const gchar *sender,
                             const gchar *object_path,
                             const gchar *interface_name,
                             const gchar *method_name, GVariant *parameters,
                             GDBusMethodInvocation *invocation,
                             gpointer user_data) {
  (void)connection;
  (void)sender;
  (void)object_path;
  (void)interface_name;
  (void)parameters;

  BridgeState *state = user_data;
  if (g_strcmp0(method_name, "GetMountPoint") == 0) {
    g_dbus_method_invocation_return_value(
        invocation, g_variant_new("(@ay)", path_bytes_variant(
                                               state->documents.mountpoint)));
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
    g_dbus_method_invocation_return_error(
        invocation, G_IO_ERROR, G_IO_ERROR_NOT_SUPPORTED,
        "%s is not implemented by this V1 bridge", method_name);
  }
}

const GDBusInterfaceVTable DOCUMENTS_VTABLE = {
    .method_call = handle_documents_method,
    .get_property = handle_get_property,
};
