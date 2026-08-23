#ifndef FILE_CHOOSER_PORTAL_H
#define FILE_CHOOSER_PORTAL_H
#include "portal_bridge_process.h"
void handle_filechooser_open(BridgeState *, const char *, GVariant *,
                             GDBusMethodInvocation *);
GVariant *rewrite_filechooser_results(BridgeState *, guint32, GVariant *);
#endif
