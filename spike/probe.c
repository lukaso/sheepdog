#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <errno.h>
#include <signal.h>
#include <sys/event.h>
#include <sys/sysctl.h>
#include <libproc.h>
int main(void){
  /* 1: kqueue NOTE_TRACK (auto-follow forks) */
  pid_t c=fork(); if(c==0){sleep(3);_exit(0);}
  int kq=kqueue(); struct kevent ev; EV_SET(&ev,c,EVFILT_PROC,EV_ADD,NOTE_EXIT|NOTE_FORK|NOTE_TRACK,0,0);
  int r=kevent(kq,&ev,1,NULL,0,NULL); printf("NOTE_TRACK: r=%d errno=%d (%s)\n",r,errno,strerror(errno));
  EV_SET(&ev,c,EVFILT_PROC,EV_ADD,NOTE_EXIT|NOTE_FORK,0,0);
  r=kevent(kq,&ev,1,NULL,0,NULL); printf("NOTE_FORK only: r=%d\n",r);
  kill(c,9);
  /* 2: read another same-user process's environment via KERN_PROCARGS2 */
  pid_t e=fork(); if(e==0){ setenv("DEEPKILL_TAG","abc123",1); char*a[]={"/bin/sleep","5",NULL}; execve(a[0],a,(char*[]){"DEEPKILL_TAG=abc123",NULL}); _exit(1);}
  usleep(200000);
  int mib[3]={CTL_KERN,KERN_PROCARGS2,e}; char buf[65536]; size_t sz=sizeof buf;
  r=sysctl(mib,3,buf,&sz,NULL,0); int found=0;
  for(size_t i=0;i+12<sz;i++) if(!memcmp(buf+i,"DEEPKILL_TAG=",13)){found=1;break;}
  printf("KERN_PROCARGS2 same-user: r=%d env tag found=%d\n",r,found);
  kill(e,9);
  /* 3: does a root-owned process expose env to us? (launchd pid 1) */
  mib[2]=1; sz=sizeof buf; r=sysctl(mib,3,buf,&sz,NULL,0); printf("KERN_PROCARGS2 pid1: r=%d errno=%d\n",r,errno);
  /* 4: proc_listallpids count */
  int n=proc_listallpids(NULL,0); printf("proc_listallpids: %d\n",n);
  return 0;
}
