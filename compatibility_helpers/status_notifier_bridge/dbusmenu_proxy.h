#ifndef DBUSMENU_PROXY_H
#define DBUSMENU_PROXY_H
#include "status_notifier_item_proxy.h"
struct _MenuProxy {
  StatusItem *item;
  char *local_path;
  char *host_path;
  guint host_registration_id;
  guint local_signal_id;
};
extern const char *DBUSMENU_XML;
void free_menu_proxy(MenuProxy *);
MenuProxy *ensure_menu_proxy(StatusItem *, const char *);
#endif
