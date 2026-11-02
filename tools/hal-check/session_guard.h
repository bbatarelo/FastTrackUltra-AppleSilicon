/* Offline-testable continuity gate for qualification tools. A recovered stream
 * is a discontinuity even if it returns to healthy before the next poll. */
#ifndef FTU_SESSION_GUARD_H
#define FTU_SESSION_GUARD_H
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
#include <ctype.h>
#include <limits.h>
struct session_guard { uint64_t epoch; bool valid; };
static bool session_uint(const char *json, const char *key, uint64_t *out) {
    const char *p=strstr(json,key); if(!p)return false; p+=strlen(key);
    while(isspace((unsigned char)*p))p++;
    if(*p++!=':')return false;
    while(isspace((unsigned char)*p))p++;
    if(*p<'0'||*p>'9')return false;
    uint64_t n=0;
    do { unsigned digit=(unsigned)(*p++-'0');if(n>(UINT64_MAX-digit)/10)return false;n=n*10+digit; }while(*p>='0'&&*p<='9');
    while(isspace((unsigned char)*p))p++;
    if(*p!=',' && *p!='}')return false;
    *out=n;return true;
}
static struct session_guard session_guard_before_start(const char *status) {
    uint64_t epoch=0,phase=0;
    if(!session_uint(status,"\"epoch\"",&epoch)||!session_uint(status,"\"phase\"",&phase)||epoch==UINT64_MAX||(phase!=0&&phase!=2))return (struct session_guard){0,false};
    /* An already running shared stream keeps its epoch; every fresh start has
     * exactly one new epoch. Recovery during Start is not an accepted trial. */
    return (struct session_guard){epoch+(phase!=2),true};
}
static bool session_guard_matches(struct session_guard g,const char *status) {
    uint64_t epoch=0;return g.valid && session_uint(status,"\"epoch\"",&epoch) && epoch==g.epoch;
}
#endif
