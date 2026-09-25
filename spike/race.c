#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <fcntl.h>
#include <libproc.h>
#include <sys/event.h>
#include <sys/wait.h>
#include <stdint.h>
struct uq { uint8_t u[16]; uint64_t uniq; uint64_t puniq; int32_t a,b; uint64_t r2,r3; };
#define MAXK 100000
static uint64_t known[MAXK]; static int nk=0;
static int isk(uint64_t x){ for(int i=0;i<nk;i++) if(known[i]==x) return 1; return 0; }
static pid_t pl[20000];
static int kq;
static int scan(void){ int added=0, again=1; while(again){ again=0; int n=proc_listallpids(pl,sizeof pl)/1; 
  for(int i=0;i<n;i++){ struct uq u; if(proc_pidinfo(pl[i],17,0,&u,sizeof u)!=sizeof u) continue; if(isk(u.puniq)&&!isk(u.uniq)){ known[nk++]=u.uniq; added++; again=1; struct kevent ev; EV_SET(&ev,pl[i],EVFILT_PROC,EV_ADD,NOTE_FORK|NOTE_EXIT,0,0); kevent(kq,&ev,1,NULL,0,NULL);} } }
  return added; }
int main(int argc,char**argv){ int N=argc>1?atoi(argv[1]):300; int mode=argc>2?atoi(argv[2]):1; /* mode1: C does setsid */
 int fd[2]; pipe(fd); kq=kqueue(); struct uq me; proc_pidinfo(getpid(),17,0,&me,sizeof me); known[nk++]=me.uniq;
 pid_t R=fork(); if(R==0){ close(fd[0]); for(int i=0;i<N;i++){ pid_t c=fork(); if(c==0){ if(mode) setsid(); pid_t g=fork(); if(g==0){ execl("/bin/sleep","sleep","4",(char*)0); _exit(1);} write(fd[1],&g,sizeof g); _exit(0);} waitpid(c,0,0);} _exit(0);} 
 close(fd[1]); struct kevent ev; EV_SET(&ev,R,EVFILT_PROC,EV_ADD,NOTE_FORK|NOTE_EXIT,0,0); kevent(kq,&ev,1,NULL,0,NULL);
 struct uq ru; if(proc_pidinfo(R,17,0,&ru,sizeof ru)==sizeof ru) known[nk++]=ru.uniq;
 int scans=0; struct timespec to={0,20000000}; int rdone=0;
 while(!rdone){ struct kevent out[64]; int k=kevent(kq,NULL,0,out,64,&to); for(int i=0;i<k;i++) if(out[i].ident==(uintptr_t)R && (out[i].fflags&NOTE_EXIT)) rdone=1; scan(); scans++; }
 waitpid(R,0,0); scan();
 fcntl(fd[0],F_SETFL,O_NONBLOCK); pid_t g; int total=0, lost=0; pid_t gs[20000];
 while(read(fd[0],&g,sizeof g)==sizeof g){ gs[total++]=g; struct uq u; if(proc_pidinfo(g,17,0,&u,sizeof u)!=sizeof u || !isk(u.uniq)) lost++; }
 printf("mode(setsid)=%d N=%d scans=%d grandchildren=%d LOST=%d known=%d\n",mode,N,scans,total,lost,nk);
 for(int i=0;i<total;i++) kill(gs[i],9);
 return 0;}
