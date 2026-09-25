#include <stdio.h>
#include <stdlib.h>
#include <signal.h>
#include <unistd.h>
#include <fcntl.h>
#include <errno.h>
#include <string.h>
#include <termios.h>
#include <sys/wait.h>
static volatile int hits=0;
static void h(int s){ hits++; if(hits==1){ kill(getpid(),SIGTSTP); /* 2nd TSTP while handler runs (vim-style kill(0)) */ raise(SIGSTOP);} }
static char st(pid_t p){ char cmd[64],b[16]={0}; snprintf(cmd,64,"ps -o stat= -p %d",p); FILE*f=popen(cmd,"r"); fgets(b,16,f); pclose(f); return b[0]; }
int main(){
 /* A: orphaned pgrp (new session): default TSTP vs supervisor-style handler that self-SIGSTOPs */
 for(int m=0;m<2;m++){ pid_t c=fork(); if(!c){ setsid(); if(m) signal(SIGTSTP,h); for(int i=0;i<50;i++) usleep(100000); _exit(0);}
   usleep(150000); kill(c,SIGTSTP); usleep(300000);
   printf("A orphaned pgrp, %s: state after TSTP = %c\n", m?"handler raises SIGSTOP":"default TSTP", st(c)); kill(c,SIGKILL); waitpid(c,0,0);}
 /* B: no controlling tty at all */
 pid_t c=fork(); if(!c){ setsid(); int f=open("/dev/tty",O_RDWR); int e=errno; printf("B no ctty: open /dev/tty -> %d errno=%s; tcgetpgrp(0) -> %d errno=%s\n", f, strerror(e), (int)tcgetpgrp(0), strerror(errno)); _exit(0);} waitpid(c,0,0);
 /* C: 2nd TSTP pending while handler (TSTP blocked) self-stops; then CONT: is it discarded? */
 int pp[2]; pipe(pp);
 c=fork(); if(!c){ hits=0; struct sigaction sa={0}; sa.sa_handler=h; sigemptyset(&sa.sa_mask); sigaction(SIGTSTP,&sa,0);
   raise(SIGTSTP); usleep(300000); char b='0'+hits; write(pp[1],&b,1); _exit(0);}
 usleep(200000); printf("C state while self-stopped = %c\n", st(c)); kill(c,SIGCONT); usleep(100000);
 printf("C state 100ms after CONT = %c\n", st(c)); kill(c,SIGCONT); char b='?'; 
 fcntl(pp[0],F_SETFL,O_NONBLOCK); usleep(400000); read(pp[0],&b,1); printf("C handler entries = %c\n", b); kill(c,SIGKILL); waitpid(c,0,0);
 return 0;}
