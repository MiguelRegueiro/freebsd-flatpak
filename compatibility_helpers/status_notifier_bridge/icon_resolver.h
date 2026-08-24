#ifndef ICON_RESOLVER_H
#define ICON_RESOLVER_H
#include "status_notifier_watcher.h"

GVariant *resolve_status_icon(StatusNotifierBridge *, const char *,
                              const char *);

#endif
