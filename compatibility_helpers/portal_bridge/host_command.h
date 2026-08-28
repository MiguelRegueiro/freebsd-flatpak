#ifndef FREEBSD_FLATPAK_HOST_COMMAND_H
#define FREEBSD_FLATPAK_HOST_COMMAND_H

#include <gio/gio.h>
#include <stdbool.h>

typedef struct _HostCommandService HostCommandService;

struct _HostCommandService {
  GDBusConnection *connection;
  GDBusNodeInfo *node;
  GHashTable *commands;
  char **host_environment;
  guint registration_id;
};

extern const char HOST_COMMAND_XML[];

bool host_command_service_init(HostCommandService *service,
                               const char *host_bus_address, GError **error);
bool host_command_service_register(HostCommandService *service,
                                   GDBusConnection *connection,
                                   GError **error);
void host_command_service_close_client(HostCommandService *service,
                                       const char *client_sender);
void host_command_service_cleanup(HostCommandService *service);
void host_command_service_clear(HostCommandService *service);

#endif
