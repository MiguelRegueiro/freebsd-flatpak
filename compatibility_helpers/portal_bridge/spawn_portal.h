#ifndef FREEBSD_FLATPAK_SPAWN_PORTAL_H
#define FREEBSD_FLATPAK_SPAWN_PORTAL_H

#include "portal_bridge_process.h"

#define FLATPAK_PORTAL_BUS_NAME "org.freedesktop.portal.Flatpak"
#define FLATPAK_PORTAL_PATH "/org/freedesktop/portal/Flatpak"
#define FLATPAK_PORTAL_INTERFACE "org.freedesktop.portal.Flatpak"
#define SPAWN_AGENT_INTERFACE "org.freebsd.Flatpak.SpawnAgent"
#define SPAWN_AGENT_PATH "/org/freebsd/Flatpak/SpawnAgent"

extern const char *FLATPAK_PORTAL_XML;
extern const GDBusInterfaceVTable FLATPAK_PORTAL_VTABLE;

bool spawn_portal_add_sandbox(BridgeState *state, const char *root,
                              const char *agent_name, GError **error);
void spawn_portal_remove_sandbox(BridgeState *state, const char *root);
void spawn_portal_close_client(BridgeState *state, const char *sender);
void spawn_portal_cleanup(BridgeState *state);
void spawn_portal_subscribe_agent_signals(BridgeState *state);
void spawn_portal_initialize(BridgeState *state);

#endif
