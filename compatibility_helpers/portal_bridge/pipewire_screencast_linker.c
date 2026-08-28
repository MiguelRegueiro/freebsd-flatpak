#include "pipewire_screencast_linker.h"
#include "portal_bridge_process.h"
void pipewire_compat_try_links(PipeWireCompat *compat);

uint32_t parse_pipewire_id(const char *value) {
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

uint64_t parse_pipewire_serial(const char *value) {
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

ScreenCastSource *session_source_for_id(SessionRecord *session,
                                        uint32_t node_id) {
  if (session == NULL || session->sources == NULL) {
    return NULL;
  }
  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
    if (source->node_id == node_id) {
      return source;
    }
  }
  return NULL;
}

bool session_approves_source(SessionRecord *session, uint32_t node_id) {
  return session_source_for_id(session, node_id) != NULL;
}

bool source_generation_matches(const ScreenCastSource *source,
                               const PipeWireNode *node) {
  return source != NULL && node != NULL && source->node_id == node->id &&
         (source->serial == 0 || node->serial == 0 ||
          source->serial == node->serial);
}

void remove_session_source_for_node(SessionRecord *session,
                                    const PipeWireNode *node) {
  if (session == NULL || session->sources == NULL || node == NULL) {
    return;
  }
  for (guint i = session->sources->len; i > 0; i--) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i - 1);
    if (source_generation_matches(source, node)) {
      g_array_remove_index(session->sources, i - 1);
    }
  }
}

void free_pipewire_client(PipeWireClient *client) {
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

void free_pipewire_node(PipeWireNode *node) {
  if (node == NULL) {
    return;
  }
  g_free(node->media_class);
  g_free(node->target_object);
  g_free(node);
}

void free_pipewire_port(PipeWirePort *port) { g_free(port); }

void free_pipewire_link(PipeWireLink *link) {
  if (link == NULL) {
    return;
  }
  if (link->proxy != NULL) {
    spa_hook_remove(&link->proxy_listener);
    pw_proxy_destroy(link->proxy);
  }
  g_free(link);
}

PipeWireNode *find_pipewire_node(PipeWireCompat *compat, uint32_t id) {
  for (guint i = 0; i < compat->nodes->len; i++) {
    PipeWireNode *node = g_ptr_array_index(compat->nodes, i);
    if (node->id == id) {
      return node;
    }
  }
  return NULL;
}

bool pipewire_client_permission(PipeWireClient *client, uint32_t object_id,
                                uint32_t *out_permissions) {
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

bool pipewire_client_is_restricted(PipeWireClient *client) {
  uint32_t default_permissions = 0;
  return client->is_portal && client->permissions_received &&
         pipewire_client_permission(client, PW_ID_ANY, &default_permissions) &&
         default_permissions == 0;
}

bool pipewire_client_matches_session(PipeWireClient *client,
                                     SessionRecord *session) {
  if (!pipewire_client_is_restricted(client) || session == NULL ||
      session->closed || session->close_requested || session->sources == NULL ||
      session->sources->len == 0) {
    return false;
  }

  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
    uint32_t permissions = 0;
    if (!pipewire_client_permission(client, source->node_id, &permissions) ||
        (permissions & PW_PERM_R) == 0) {
      return false;
    }
  }

  BridgeState *state = session->state;
  for (guint i = 0; i < state->screencast.sessions->len; i++) {
    SessionRecord *other = g_ptr_array_index(state->screencast.sessions, i);
    if (other == session || other->sources == NULL) {
      continue;
    }
    for (guint j = 0; j < other->sources->len; j++) {
      ScreenCastSource *source =
          &g_array_index(other->sources, ScreenCastSource, j);
      uint32_t permissions = 0;
      if (!session_approves_source(session, source->node_id) &&
          pipewire_client_permission(client, source->node_id, &permissions) &&
          (permissions & PW_PERM_R) != 0) {
        return false;
      }
    }
  }
  return true;
}

bool source_node_is_approved(SessionRecord *session, PipeWireNode *node) {
  return source_generation_matches(session_source_for_id(session, node->id),
                                   node);
}

PipeWireNode *source_node_for_consumer(PipeWireCompat *compat,
                                       SessionRecord *session,
                                       PipeWireNode *consumer) {
  if (session->sources->len == 1) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, 0);
    PipeWireNode *node = find_pipewire_node(compat, source->node_id);
    return node != NULL && source_node_is_approved(session, node) ? node : NULL;
  }

  if (consumer->target_object == NULL) {
    return NULL;
  }
  uint64_t target = parse_pipewire_serial(consumer->target_object);
  for (guint i = 0; i < session->sources->len; i++) {
    ScreenCastSource *source =
        &g_array_index(session->sources, ScreenCastSource, i);
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

bool pipewire_link_exists(PipeWireCompat *compat, SessionRecord *session,
                          uint32_t source_port_id, uint32_t consumer_port_id) {
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

void on_pipewire_link_destroy(void *user_data) {
  PipeWireLink *link = user_data;
  spa_hook_remove(&link->proxy_listener);
  link->proxy = NULL;
}

void on_pipewire_link_removed(void *user_data) {
  PipeWireLink *link = user_data;
  PipeWireCompat *compat = link->compat;
  pw_proxy_destroy(link->proxy);
  g_ptr_array_remove(compat->links, link);
}

void on_pipewire_link_error(void *user_data, int seq, int result,
                            const char *message) {
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

void create_pipewire_link(PipeWireCompat *compat, SessionRecord *session,
                          PipeWireClient *client, PipeWireNode *source,
                          PipeWirePort *source_port, PipeWireNode *consumer,
                          PipeWirePort *consumer_port) {
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
      PW_KEY_LINK_OUTPUT_NODE, source_node_id, PW_KEY_LINK_OUTPUT_PORT,
      source_port_id, PW_KEY_LINK_INPUT_NODE, consumer_node_id,
      PW_KEY_LINK_INPUT_PORT, consumer_port_id, PW_KEY_OBJECT_LINGER, "false",
      NULL);
  struct pw_proxy *proxy = pw_core_create_object(
      compat->core, "link-factory", PW_TYPE_INTERFACE_Link, PW_VERSION_LINK,
      &properties->dict, 0);
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
  diagnostic_line(
      "linked approved ScreenCast source %u:%u -> portal client %u node %u:%u",
      source->id, source_port->id, client->id, consumer->id, consumer_port->id);
}

void pipewire_compat_try_links(PipeWireCompat *compat) {
  if (compat == NULL || compat->core == NULL) {
    return;
  }
  for (guint client_index = 0; client_index < compat->clients->len;
       client_index++) {
    PipeWireClient *client = g_ptr_array_index(compat->clients, client_index);
    for (guint session_index = 0;
         session_index < compat->state->screencast.sessions->len;
         session_index++) {
      SessionRecord *session =
          g_ptr_array_index(compat->state->screencast.sessions, session_index);
      if (!pipewire_client_matches_session(client, session)) {
        continue;
      }
      for (guint node_index = 0; node_index < compat->nodes->len;
           node_index++) {
        PipeWireNode *consumer = g_ptr_array_index(compat->nodes, node_index);
        if (consumer->client_id != client->id ||
            g_strcmp0(consumer->media_class, "Stream/Input/Video") != 0) {
          continue;
        }
        PipeWireNode *source =
            source_node_for_consumer(compat, session, consumer);
        if (source == NULL) {
          continue;
        }
        for (guint out_index = 0; out_index < compat->ports->len; out_index++) {
          PipeWirePort *output = g_ptr_array_index(compat->ports, out_index);
          if (!output->is_output || output->node_id != source->id) {
            continue;
          }
          for (guint in_index = 0; in_index < compat->ports->len; in_index++) {
            PipeWirePort *input = g_ptr_array_index(compat->ports, in_index);
            if (input->is_input && input->node_id == consumer->id) {
              create_pipewire_link(compat, session, client, source, output,
                                   consumer, input);
            }
          }
        }
      }
    }
  }
}

void remove_pipewire_links_for_session(SessionRecord *session) {
  PipeWireCompat *compat =
      session != NULL ? session->state->screencast.pipewire : NULL;
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

void remove_pipewire_links_for_object(PipeWireCompat *compat,
                                      uint32_t object_id, bool client) {
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

void on_pipewire_client_permissions(void *user_data, uint32_t index,
                                    uint32_t n_permissions,
                                    const struct pw_permission *permissions) {
  PipeWireClient *client = user_data;
  if (index == 0) {
    g_array_set_size(client->permissions, 0);
  }
  if (index > client->permissions->len) {
    g_array_set_size(client->permissions, index);
  }
  for (uint32_t i = 0; i < n_permissions; i++) {
    if (index + i < client->permissions->len) {
      g_array_index(client->permissions, struct pw_permission, index + i) =
          permissions[i];
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

void refresh_pipewire_client_permissions(PipeWireClient *client) {
  if (client == NULL || client->proxy == NULL) {
    return;
  }
  int result = pw_client_get_permissions(client->proxy, 0, UINT32_MAX);
  if (result < 0) {
    log_line("read PipeWire portal client %u permissions failed: %s",
             client->id, spa_strerror(result));
  }
}

void refresh_pipewire_permissions_for_client(PipeWireCompat *compat,
                                             uint32_t client_id) {
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

void on_pipewire_registry_global(void *user_data, uint32_t id,
                                 uint32_t permissions, const char *type,
                                 uint32_t version,
                                 const struct spa_dict *properties) {
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
    client->permissions =
        g_array_new(FALSE, TRUE, sizeof(struct pw_permission));
    client->proxy =
        pw_registry_bind(compat->registry, id, PW_TYPE_INTERFACE_Client,
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
    node->client_id =
        parse_pipewire_id(spa_dict_lookup(properties, PW_KEY_CLIENT_ID));
    node->serial = parse_pipewire_serial(
        spa_dict_lookup(properties, PW_KEY_OBJECT_SERIAL));
    node->media_class =
        g_strdup(spa_dict_lookup(properties, PW_KEY_MEDIA_CLASS));
    node->target_object =
        g_strdup(spa_dict_lookup(properties, PW_KEY_TARGET_OBJECT));
    g_ptr_array_add(compat->nodes, node);
    if (g_strcmp0(node->media_class, "Stream/Input/Video") == 0) {
      refresh_pipewire_permissions_for_client(compat, node->client_id);
    }
    pipewire_compat_try_links(compat);
  } else if (g_strcmp0(type, PW_TYPE_INTERFACE_Port) == 0) {
    PipeWirePort *port = g_new0(PipeWirePort, 1);
    port->id = id;
    port->node_id =
        parse_pipewire_id(spa_dict_lookup(properties, PW_KEY_NODE_ID));
    const char *direction = spa_dict_lookup(properties, PW_KEY_PORT_DIRECTION);
    port->is_input = g_strcmp0(direction, "in") == 0;
    port->is_output = g_strcmp0(direction, "out") == 0;
    g_ptr_array_add(compat->ports, port);
    pipewire_compat_try_links(compat);
  }
}

void on_pipewire_registry_global_remove(void *user_data, uint32_t id) {
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
         session_index < compat->state->screencast.sessions->len;
         session_index++) {
      SessionRecord *session =
          g_ptr_array_index(compat->state->screencast.sessions, session_index);
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

void on_pipewire_core_error(void *user_data, uint32_t id, int seq, int result,
                            const char *message) {
  (void)user_data;
  (void)seq;
  if (id == PW_ID_CORE) {
    log_line("PipeWire compatibility connection failed: %s (%s)", message,
             spa_strerror(result));
  }
}

static const struct pw_core_events PIPEWIRE_CORE_EVENTS = {
    PW_VERSION_CORE_EVENTS,
    .error = on_pipewire_core_error,
};

gboolean pipewire_source_prepare(GSource *source, gint *timeout) {
  (void)source;
  *timeout = -1;
  return FALSE;
}

gboolean pipewire_source_dispatch(GSource *source, GSourceFunc callback,
                                  gpointer user_data) {
  (void)callback;
  (void)user_data;
  PipeWireSource *pipewire_source = (PipeWireSource *)source;
  int result =
      pw_loop_iterate(pw_main_loop_get_loop(pipewire_source->compat->loop), 0);
  if (result < 0) {
    log_line("PipeWire compatibility loop failed: %s", spa_strerror(result));
  }
  return G_SOURCE_CONTINUE;
}

void pipewire_source_finalize(GSource *source) {
  PipeWireSource *pipewire_source = (PipeWireSource *)source;
  pw_loop_leave(pw_main_loop_get_loop(pipewire_source->compat->loop));
}

static GSourceFuncs PIPEWIRE_SOURCE_FUNCS = {
    .prepare = pipewire_source_prepare,
    .dispatch = pipewire_source_dispatch,
    .finalize = pipewire_source_finalize,
};

void free_pipewire_compat(PipeWireCompat *compat) {
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

PipeWireCompat *new_pipewire_compat(BridgeState *state) {
  pw_init(NULL, NULL);
  PipeWireCompat *compat = g_new0(PipeWireCompat, 1);
  compat->state = state;
  compat->clients =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_client);
  compat->nodes =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_node);
  compat->ports =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_port);
  compat->links =
      g_ptr_array_new_with_free_func((GDestroyNotify)free_pipewire_link);
  compat->loop = pw_main_loop_new(NULL);
  if (compat->loop == NULL) {
    free_pipewire_compat(compat);
    return NULL;
  }
  compat->context =
      pw_context_new(pw_main_loop_get_loop(compat->loop), NULL, 0);
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
  compat->registry = pw_core_get_registry(compat->core, PW_VERSION_REGISTRY, 0);
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
  diagnostic_line("enabled ownership-based PipeWire ScreenCast linking");
  return compat;
}
