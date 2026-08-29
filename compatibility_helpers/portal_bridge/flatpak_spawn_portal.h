#ifndef FREEBSD_FLATPAK_FLATPAK_SPAWN_PORTAL_H
#define FREEBSD_FLATPAK_FLATPAK_SPAWN_PORTAL_H

#include "portal_bridge_process.h"

extern const char FLATPAK_SPAWN_XML[];
extern const GDBusInterfaceVTable FLATPAK_SPAWN_VTABLE;
void flatpak_spawn_watch_lifecycle(BridgeState *state, int fd, guint32 request,
                                   guint32 pid, const char *sender);
void flatpak_spawn_cleanup_lifecycles(BridgeState *state);
void flatpak_spawn_lifecycle_free(gpointer lifecycle);

#endif
