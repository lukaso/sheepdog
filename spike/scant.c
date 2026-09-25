#include <stdio.h>
#include <libproc.h>
#include <stdint.h>
#include <time.h>
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a,b; uint64_t r2,r3; };
int main(){ static pid_t pl[20000]; struct timespec a,b; clock_gettime(CLOCK_MONOTONIC,&a); int ok=0,n=0;
 for(int k=0;k<20;k++){ n=proc_listallpids(pl,sizeof pl); ok=0; for(int i=0;i<n;i++){ struct uq u; if(proc_pidinfo(pl[i],17,0,&u,sizeof u)==sizeof u) ok++; } }
 clock_gettime(CLOCK_MONOTONIC,&b); printf("pids=%d answered=%d per-scan=%.2f ms\n",n,ok,((b.tv_sec-a.tv_sec)*1e3+(b.tv_nsec-a.tv_nsec)/1e6)/20); }
