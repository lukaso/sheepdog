// A: who gets HUP when the pty master closes. L = session leader (like deepkill exec'd by `sh -c`, tmux, sshd, script).
// mode 0: C in L's group (deepkill's root). mode 1: C in its own group made foreground (a shell's job).
#define _XOPEN_SOURCE 700
#define _DARWIN_C_SOURCE
#define _DEFAULT_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <fcntl.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/wait.h>
#include <termios.h>
static int fd; static const char *who;
static void h(int s){ char b[64]; int n=snprintf(b,64,"%s got %s pid=%d\n",who,s==SIGHUP?"HUP":"CONT",getpid()); write(fd,b,n);}
int main(int ac,char**av){ int mode=atoi(av[1]); fd=open(av[2],O_WRONLY|O_CREAT|O_APPEND,0600);
 int m=posix_openpt(O_RDWR|O_NOCTTY); grantpt(m); unlockpt(m); char *sn=ptsname(m);
 pid_t L=fork(); if(!L){ close(m); setsid(); int s=open(sn,O_RDWR); ioctl(s,TIOCSCTTY,0); who="L";
   signal(SIGHUP,h); signal(SIGCONT,h);
   pid_t C=fork(); if(!C){ who="C"; if(mode==1){ setpgid(0,0);} signal(SIGHUP,h); signal(SIGCONT,h); for(int i=0;i<30;i++) sleep(1); _exit(0);} 
   if(mode==1){ setpgid(C,C); signal(SIGTTOU,SIG_IGN); tcsetpgrp(s,C);} 
   char b[64]; int n=snprintf(b,64,"L=%d C=%d fg=%d\n",getpid(),C,tcgetpgrp(s)); write(fd,b,n);
   for(int i=0;i<3;i++) sleep(1); kill(C,SIGKILL); _exit(0);} 
 usleep(400000); close(m); usleep(800000); 
 int st; waitpid(L,&st,0); return 0; }
