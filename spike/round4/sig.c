/* child in a pty: prints siginfo for INT and HUP, and tcgetpgrp results */
#include <stdio.h>
#include <signal.h>
#include <unistd.h>
#include <fcntl.h>
#include <string.h>
#include <errno.h>
#include <termios.h>
static volatile sig_atomic_t got=0,code=0,spid=0,num=0;
static void h(int s, siginfo_t*i, void*u){ num=s; code=i->si_code; spid=i->si_pid; got++; }
int main(){ int log=open("/private/tmp/claude-501/dk-review4/sig.log",O_WRONLY|O_CREAT|O_APPEND,0600);
 int tty=open("/dev/tty",O_RDWR); dprintf(log,"start pid=%d pgrp=%d tcgetpgrp(/dev/tty)=%d\n",getpid(),getpgrp(),tcgetpgrp(tty));
 struct sigaction sa={0}; sa.sa_sigaction=h; sa.sa_flags=SA_SIGINFO; sigaction(SIGINT,&sa,0); sigaction(SIGHUP,&sa,0);
 int last=0; for(int k=0;k<60;k++){ usleep(100000); if(got!=last){ last=got; errno=0; int fg=tcgetpgrp(tty); int e=errno; errno=0; int t2=open("/dev/tty",O_RDWR); int e2=errno;
   dprintf(log,"sig=%d si_code=%d si_pid=%d | tcgetpgrp(old fd)=%d errno=%s | reopen /dev/tty=%d errno=%s | getpgrp=%d\n",num,code,spid,fg,strerror(e),t2,strerror(e2),getpgrp()); if(t2>=0) close(t2);} }
 return 0;}
