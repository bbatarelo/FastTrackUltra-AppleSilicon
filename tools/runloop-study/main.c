/* Standalone macOS delivery experiment. No USB, HAL, audio or driver access. */
#include <CoreFoundation/CoreFoundation.h>
#include <mach/mach.h>
#include <mach/mach_time.h>
#include <mach/thread_policy.h>
#include <pthread.h>
#include <stdbool.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define MAX_PUMPS 65536
#define MAX_PAIRS 2500
struct message { mach_msg_header_t header; uint64_t sequence, endpoint; };
struct sent { uint64_t planned, begin, end; int32_t status; };
struct received { uint64_t sequence, endpoint, ticks, pump; };
struct pump { uint64_t begin, end, before, after, waits, wait_ticks, callbacks; int32_t result; };
struct sample { uint64_t begin, end, cpu_ns; int32_t cpu_status, policy_status, basic; boolean_t defaults; thread_time_constraint_policy_data_t policy; };
struct study;
struct endpoint { struct study *study; unsigned index; mach_port_t port; bool send_right; CFMachPortRef cf; CFRunLoopSourceRef source; };
struct study {
    unsigned pairs, idle_seconds, received, pumps, errors, observer_errors;
    bool realtime, observing, in_pump;
    uint64_t start, stop, producer_base, wait_start;
    mach_timebase_info_data_t tb;
    struct endpoint ep[2];
    struct sent *sent;
    struct received *records;
    struct pump *pump;
    struct sample first, last, producer_policy;
    int32_t configure_status, restore_status, setup_status, producer_configure_status;
};
static uint64_t ticks(struct study *s, uint64_t ns) { return ns*s->tb.denom/s->tb.numer; }
static uint64_t cpu_ns(time_value_t t) { return (uint64_t)t.seconds*1000000000ULL+(uint64_t)t.microseconds*1000; }
static void sample(struct sample *s)
{
    thread_t thread=mach_thread_self();
    thread_basic_info_data_t basic={0};
    mach_msg_type_number_t count=THREAD_BASIC_INFO_COUNT;
    s->begin=mach_absolute_time();
    s->cpu_status=thread_info(thread,THREAD_BASIC_INFO,(thread_info_t)&basic,&count);
    if(!s->cpu_status){s->cpu_ns=cpu_ns(basic.user_time)+cpu_ns(basic.system_time);s->basic=basic.policy;}
    count=THREAD_TIME_CONSTRAINT_POLICY_COUNT;s->defaults=false;
    s->policy_status=thread_policy_get(thread,THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&s->policy,&count,&s->defaults);
    s->end=mach_absolute_time();
    mach_port_deallocate(mach_task_self(),thread);
}
static void callback(CFMachPortRef port, void *data, CFIndex size, void *info)
{
    (void)port;
    struct endpoint *ep=info;struct study *s=ep->study;
    struct message *m=data;
    uint64_t now=mach_absolute_time();
    if(size<(CFIndex)sizeof(*m) || m->header.msgh_id!=0x465455 || m->header.msgh_size!=sizeof(*m)
       || m->endpoint!=ep->index || m->sequence>=s->pairs || !s->in_pump || s->received>=2*s->pairs){s->errors++;return;}
    s->records[s->received++]=(struct received){m->sequence,m->endpoint,now,s->pumps};
    s->pump[s->pumps].callbacks++;
}
static void observe(CFRunLoopObserverRef observer, CFRunLoopActivity activity, void *info)
{
    (void)observer;struct study *s=info;uint64_t now=mach_absolute_time();
    if(!s->in_pump){s->observer_errors++;return;}
    struct pump *p=&s->pump[s->pumps];
    if(activity==kCFRunLoopBeforeWaiting){
        if(s->wait_start)s->observer_errors++;
        s->wait_start=now;p->before=now;
    }else if(activity==kCFRunLoopAfterWaiting){
        if(!s->wait_start){s->observer_errors++;return;}
        p->after=now;p->wait_ticks+=now-s->wait_start;p->waits++;s->wait_start=0;
    }
}
static void *produce(void *arg)
{
    struct study *s=arg;
    thread_extended_policy_data_t normal={true};
    s->producer_configure_status=thread_policy_set(pthread_mach_thread_np(pthread_self()),THREAD_EXTENDED_POLICY,(thread_policy_t)&normal,THREAD_EXTENDED_POLICY_COUNT);
    sample(&s->producer_policy);
    for(unsigned n=0;n<s->pairs;n++){
        uint64_t planned=s->producer_base+ticks(s,(uint64_t)n*8000000);
        kern_return_t wait_status=mach_wait_until(planned);
        for(unsigned e=0;e<2;e++){
            struct sent *r=&s->sent[2*n+e];r->planned=planned;
            struct message m={0};m.sequence=n;m.endpoint=e;
            m.header.msgh_bits=MACH_MSGH_BITS(MACH_MSG_TYPE_COPY_SEND,0);
            m.header.msgh_size=sizeof(m);m.header.msgh_remote_port=s->ep[e].port;m.header.msgh_id=0x465455;
            r->begin=mach_absolute_time();
            r->status=wait_status?wait_status:mach_msg(&m.header,MACH_SEND_MSG|MACH_SEND_TIMEOUT,sizeof(m),0,MACH_PORT_NULL,1,MACH_PORT_NULL);
            r->end=mach_absolute_time();
        }
    }
    return NULL;
}
static void touch(void *data,size_t size)
{
    volatile unsigned char *p=data;
    for(size_t i=0;i<size;i+=4096)p[i]=0;
    if(size)p[size-1]=0;
}
static void *consume(void *arg)
{
    struct study *s=arg;CFRunLoopRef loop=CFRunLoopGetCurrent();CFRunLoopObserverRef observer=NULL;
    pthread_t producer;bool started=false,configured=false;
    for(unsigned e=0;e<2;e++){
        struct endpoint *p=&s->ep[e];p->study=s;p->index=e;
        s->setup_status=mach_port_allocate(mach_task_self(),MACH_PORT_RIGHT_RECEIVE,&p->port);
        if(s->setup_status)goto cleanup;
        s->setup_status=mach_port_insert_right(mach_task_self(),p->port,p->port,MACH_MSG_TYPE_MAKE_SEND);
        if(s->setup_status)goto cleanup;
        p->send_right=true;
        CFMachPortContext context={0,p,NULL,NULL,NULL};Boolean free_info=false;
        p->cf=CFMachPortCreateWithPort(NULL,p->port,callback,&context,&free_info);
        if(!p->cf || free_info){s->setup_status=-1;goto cleanup;}
        p->source=CFMachPortCreateRunLoopSource(NULL,p->cf,0);
        if(!p->source){s->setup_status=-2;goto cleanup;}
        CFRunLoopAddSource(loop,p->source,kCFRunLoopDefaultMode);
    }
    if(s->observing){
        CFRunLoopObserverContext context={0,s,NULL,NULL,NULL};
        observer=CFRunLoopObserverCreate(NULL,kCFRunLoopBeforeWaiting|kCFRunLoopAfterWaiting,true,0,observe,&context);
        if(!observer){s->setup_status=-3;goto cleanup;}
        CFRunLoopAddObserver(loop,observer,kCFRunLoopDefaultMode);
    }
    touch(s->sent,2*s->pairs*sizeof(*s->sent));touch(s->records,2*s->pairs*sizeof(*s->records));touch(s->pump,MAX_PUMPS*sizeof(*s->pump));
    if(s->realtime){
        thread_time_constraint_policy_data_t p={(uint32_t)ticks(s,8000000),(uint32_t)ticks(s,1000000),(uint32_t)ticks(s,2000000),true};
        s->configure_status=thread_policy_set(pthread_mach_thread_np(pthread_self()),THREAD_TIME_CONSTRAINT_POLICY,(thread_policy_t)&p,THREAD_TIME_CONSTRAINT_POLICY_COUNT);
        if(s->configure_status)goto cleanup;
        configured=true;
    }
    sample(&s->first);
    s->start=mach_absolute_time();s->producer_base=s->start+ticks(s,100000000);
    if(!s->idle_seconds){
        s->setup_status=pthread_create(&producer,NULL,produce,s);
        if(s->setup_status)goto cleanup;
        started=true;
    }
    uint64_t deadline=s->idle_seconds?s->start+ticks(s,(uint64_t)s->idle_seconds*1000000000):s->producer_base+ticks(s,(uint64_t)s->pairs*8000000+2000000000);
    while((s->idle_seconds || s->received<2*s->pairs) && s->pumps<MAX_PUMPS && mach_absolute_time()<deadline){
        struct pump *p=&s->pump[s->pumps];p->begin=mach_absolute_time();s->in_pump=true;
        p->result=CFRunLoopRunInMode(kCFRunLoopDefaultMode,0.002,true);
        p->end=mach_absolute_time();s->in_pump=false;
        if(s->wait_start){s->observer_errors++;s->wait_start=0;}
        s->pumps++;
        if(p->result!=kCFRunLoopRunTimedOut && p->result!=kCFRunLoopRunHandledSource){s->errors++;break;}
    }
    s->stop=mach_absolute_time();sample(&s->last);
cleanup:
    if(configured){
        thread_extended_policy_data_t p={true};
        s->restore_status=thread_policy_set(pthread_mach_thread_np(pthread_self()),THREAD_EXTENDED_POLICY,(thread_policy_t)&p,THREAD_EXTENDED_POLICY_COUNT);
    }
    if(started)pthread_join(producer,NULL);
    if(observer){CFRunLoopRemoveObserver(loop,observer,kCFRunLoopDefaultMode);CFRelease(observer);}
    for(unsigned e=0;e<2;e++){
        struct endpoint *p=&s->ep[e];
        if(p->source){CFRunLoopRemoveSource(loop,p->source,kCFRunLoopDefaultMode);CFRelease(p->source);}
        if(p->cf){CFMachPortInvalidate(p->cf);CFRelease(p->cf);}
        if(p->send_right && mach_port_deallocate(mach_task_self(),p->port))s->errors++;
        if(p->port && mach_port_mod_refs(mach_task_self(),p->port,MACH_PORT_RIGHT_RECEIVE,-1))s->errors++;
    }
    return NULL;
}
static void print_sample(struct sample *s)
{
    printf("{\"begin\":%llu,\"end\":%llu,\"cpu_ns\":%llu,\"cpu_status\":%d,\"policy_status\":%d,\"basic\":%d,\"default\":%d,\"constraint\":[%u,%u,%u,%d]}",s->begin,s->end,s->cpu_ns,s->cpu_status,s->policy_status,s->basic,s->defaults,s->policy.period,s->policy.computation,s->policy.constraint,s->policy.preemptible);
}
int main(int argc,char **argv)
{
    if(argc!=4 || (strcmp(argv[1],"normal") && strcmp(argv[1],"realtime")) || (strcmp(argv[2],"plain") && strcmp(argv[2],"observer"))){fprintf(stderr,"Usage: %s normal|realtime plain|observer PAIRS(1..2500)|idle:SECONDS(1..30)\n",argv[0]);return 2;}
    bool idle=!strncmp(argv[3],"idle:",5);char *end=NULL;long count=strtol(argv[3]+(idle?5:0),&end,10);
    if(*end || count<1 || count>(idle?30:MAX_PAIRS))return 2;
    struct study s={0};if(idle)s.idle_seconds=(unsigned)count;else s.pairs=(unsigned)count;s.realtime=!strcmp(argv[1],"realtime");s.observing=!strcmp(argv[2],"observer");
    if(mach_timebase_info(&s.tb) || !s.tb.numer || !s.tb.denom)return 2;
    s.sent=calloc(s.pairs?2*s.pairs:1,sizeof(*s.sent));s.records=calloc(s.pairs?2*s.pairs:1,sizeof(*s.records));s.pump=calloc(MAX_PUMPS,sizeof(*s.pump));
    if(!s.sent || !s.records || !s.pump){free(s.sent);free(s.records);free(s.pump);return 2;}
    pthread_t consumer;int err=pthread_create(&consumer,NULL,consume,&s);
    if(err){free(s.sent);free(s.records);free(s.pump);return 2;}pthread_join(consumer,NULL);
    bool valid=!s.setup_status && !s.configure_status && !s.restore_status && !s.producer_configure_status && !s.errors && !s.observer_errors && s.received==2*s.pairs;
    for(unsigned i=0;i<2*s.pairs;i++)if(s.sent[i].status || !s.sent[i].begin)valid=false;
    printf("{\"schema\":1,\"policy\":\"%s\",\"observer\":%s,\"pairs\":%u,\"timebase\":[%u,%u],\"requested_pump_ns\":2000000,\"producer_interval_ns\":8000000,\"valid\":%s,\"setup\":%d,\"configure\":%d,\"restore\":%d,\"errors\":%u,\"observer_errors\":%u,\"start\":%llu,\"stop\":%llu,\"first\":",argv[1],s.observing?"true":"false",s.pairs,s.tb.numer,s.tb.denom,valid?"true":"false",s.setup_status,s.configure_status,s.restore_status,s.errors,s.observer_errors,s.start,s.stop);
    print_sample(&s.first);printf(",\"last\":");print_sample(&s.last);printf(",\"producer_policy\":");print_sample(&s.producer_policy);printf(",\"producer_configure\":%d,\"idle_seconds\":%u,\"producer_base\":%llu,\"sent\":[",s.producer_configure_status,s.idle_seconds,s.producer_base);
    for(unsigned i=0;i<2*s.pairs;i++){struct sent *r=&s.sent[i];printf("%s[%u,%u,%llu,%llu,%llu,%d]",i?",":"",i/2,i%2,r->planned,r->begin,r->end,r->status);}
    printf("],\"received\":[");
    for(unsigned i=0;i<s.received;i++){struct received *r=&s.records[i];printf("%s[%llu,%llu,%llu,%llu]",i?",":"",r->sequence,r->endpoint,r->ticks,r->pump);}
    printf("],\"pumps\":[");
    for(unsigned i=0;i<s.pumps;i++){struct pump *p=&s.pump[i];printf("%s[%u,%llu,%llu,%d,%llu,%llu,%llu,%llu,%llu]",i?",":"",i,p->begin,p->end,p->result,p->before,p->after,p->waits,p->wait_ticks,p->callbacks);}
    printf("]}\n");free(s.sent);free(s.records);free(s.pump);return valid?0:1;
}
