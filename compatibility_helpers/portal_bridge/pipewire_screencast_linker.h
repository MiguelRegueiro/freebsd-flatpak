#ifndef PIPEWIRE_SCREENCAST_LINKER_H
#define PIPEWIRE_SCREENCAST_LINKER_H
#include "portal_bridge_process.h"
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
  char *media_role;
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
typedef struct {
  struct spa_handle *handle;
  struct pw_proxy *proxy;
} PublishedCamera;
typedef void (*PipeWireCameraReadyCallback)(gpointer);
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
  GPtrArray *published_cameras;
  GPtrArray *camera_ready_callbacks;
  int camera_lock_fd;
  bool camera_requested;
};
bool session_approves_source(SessionRecord *, uint32_t);
bool source_node_is_approved(SessionRecord *, PipeWireNode *);
void remove_session_source_for_node(SessionRecord *, const PipeWireNode *);
bool pipewire_client_matches_session(PipeWireClient *, SessionRecord *);
void pipewire_compat_try_links(PipeWireCompat *);
void remove_pipewire_links_for_session(SessionRecord *);
void refresh_pipewire_permissions_for_client(PipeWireCompat *, uint32_t);
bool pipewire_node_is_camera(const PipeWireNode *);
bool pipewire_v4l2_caps_usable(uint32_t, uint32_t);
bool pipewire_camera_present(const PipeWireCompat *);
bool pipewire_v4l2_normalize_timestamp(int64_t, int64_t, int64_t, int64_t *);
bool pipewire_v4l2_ioctl_matches(unsigned long, unsigned long);
bool pipewire_v4l2_timestamp_is_stale(int64_t, int64_t, int64_t, int64_t);
void free_pipewire_compat(PipeWireCompat *);
bool pipewire_camera_available(const PipeWireCompat *);
void pipewire_request_camera_publication(PipeWireCompat *);
void pipewire_when_camera_published(PipeWireCompat *,
                                    PipeWireCameraReadyCallback, gpointer,
                                    GDestroyNotify);
void pipewire_camera_publication_changed(PipeWireCompat *);
bool pipewire_camera_publication_needed(const PipeWireCompat *);
PipeWireCompat *new_pipewire_compat(BridgeState *);
#endif
