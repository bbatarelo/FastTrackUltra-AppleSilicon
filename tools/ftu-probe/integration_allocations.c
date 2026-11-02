/* Scope: calls originating in completion_route.c, not opaque frameworks. */
#include <stdlib.h>
#include <stdint.h>
#include <stdatomic.h>
static _Atomic uint64_t calls;
void *ftu_audit_malloc(size_t n){atomic_fetch_add(&calls,1);return malloc(n);}
void *ftu_audit_calloc(size_t n,size_t m){atomic_fetch_add(&calls,1);return calloc(n,m);}
void *ftu_audit_realloc(void *p,size_t n){atomic_fetch_add(&calls,1);return realloc(p,n);}
void ftu_audit_free(void *p){atomic_fetch_add(&calls,1);free(p);}
uint64_t ftu_integrated_allocations(void){return atomic_load(&calls);}
