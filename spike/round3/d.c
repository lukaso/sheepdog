#include "common.h"
int main(){ init(); int n=proc_listallpids(NULL,0); pid_t*ps=malloc(sizeof(pid_t)*(n+100)); n=proc_listallpids(ps,sizeof(pid_t)*(n+100));
 int cnt[100000]={0}; for(int i=0;i<n;i++){ struct proc_bsdinfo b; if(proc_pidinfo(ps[i],PROC_PIDTBSDINFO,0,&b,sizeof b)<=0) continue; if(b.pbi_uid!=getuid()) continue; int r=resp(ps[i]); if(r>0&&r<100000) cnt[r]++; }
 for(int r=1;r<100000;r++) if(cnt[r]>=15){ char nm[256]={0}; proc_name(r,nm,sizeof nm); printf("resp=%d (%s): %d same-uid processes\n",r,nm,cnt[r]); } return 0; }
