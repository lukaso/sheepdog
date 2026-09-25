#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <libproc.h>
#include <sys/proc_info.h>
#include <stdint.h>
#define PROC_PIDUNIQIDENTIFIERINFO 17
struct U { uint8_t u[16]; uint64_t uq, puq; int32_t a,b; uint64_t r2,r3; };
int main(){ pid_t pids[8192]; int n=proc_listallpids(pids,sizeof pids)/1; uid_t me=getuid();
 static uint64_t alluq[8192]; int m=0;
 for(int i=0;i<n;i++){ struct U u; if(proc_pidinfo(pids[i],PROC_PIDUNIQIDENTIFIERINFO,0,&u,sizeof u)>0) alluq[m++]=u.uq; }
 int cnt=0;
 for(int i=0;i<n;i++){ struct proc_bsdinfo b; struct U u; if(proc_pidinfo(pids[i],PROC_PIDTBSDINFO,0,&b,sizeof b)<=0) continue;
  if(b.pbi_uid!=me||b.pbi_ppid!=1) continue; if(proc_pidinfo(pids[i],PROC_PIDUNIQIDENTIFIERINFO,0,&u,sizeof u)<=0) continue;
  if(u.puq==1) continue; int alive=0; for(int j=0;j<m;j++) if(alluq[j]==u.puq) alive=1;
  char path[PROC_PIDPATHINFO_MAXSIZE]; path[0]=0; proc_pidpath(pids[i],path,sizeof path);
  printf("pid=%d uniq=%llu puniq=%llu puniq_alive=%d %s\n",pids[i],u.uq,u.puq,alive,path); cnt++; }
 printf("TOTAL strays by rule: %d\n",cnt); }
