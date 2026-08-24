#ifndef DOCUMENT_ID_H
#define DOCUMENT_ID_H

#include "portal_bridge_process.h"

char *generate_document_id(BridgeState *);
bool document_id_is_valid(const char *);

#endif
