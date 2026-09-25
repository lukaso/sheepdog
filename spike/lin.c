#define _GNU_SOURCE
#include <stdio.h>
#include <stdlib.h>
#include <unistd.h>
#include <signal.h>
#include <errno.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/wait.h>
static char st(pid_t p){ char b[64],s[512]; snprintf(b,64,"/proc/%d/stat",p); FILE*f=fopen(b,"r"); if(!f) return '-'; fgets(s,512,f); fclose(f); char*q=strrchr(s,')'); return q?q[2]:'?'; }
int main(int argc,char**argv){ if(argc>1){ int v=0; prctl(PR_GET_CHILD_SUBREAPER,&v,0,0,0); printf("subreaper after exec=%d\n",v); return 0;}
 /* cell-6 control */
 int fd[2]; pipe(fd);
 pid_t p=fork(); if(p==0){ setsid(); pid_t c=fork(); if(c==0){ raise(SIGSTOP); sleep(3); _exit(0);} write(fd[1],&c,sizeof c); usleep(200000); _exit(0);} 
 pid_t c; read(fd[0],&c,sizeof c); waitpid(p,0,0); usleep(300000); printf("cell6 control: stopped orphan state=%c\n",st(c)); kill(c,9);
 /* pidfd */
 pid_t s=fork(); if(s==0){ pause(); _exit(0);} long fdp=syscall(SYS_pidfd_open,s,0); printf("pidfd_open=%ld errno=%d\n",fdp,fdp<0?errno:0);
 long r=syscall(SYS_pidfd_send_signal,(int)fdp,SIGKILL,NULL,0); printf("pidfd_send_signal=%ld errno=%d\n",r,r<0?errno:0); waitpid(s,0,0);
 /* subreaper: zombie accumulation if supervisor does not reap */
 prctl(PR_SET_CHILD_SUBREAPER,1,0,0,0);
 pid_t m=fork(); if(m==0){ for(int i=0;i<5;i++){ if(fork()==0){ if(fork()==0){ _exit(0);} _exit(0);} } sleep(1); _exit(0);} 
 sleep(2); waitpid(m,0,0); char cmd[128]; snprintf(cmd,128,"ps -o pid,ppid,stat,comm 2>/dev/null | awk '$2==%d'",getpid()); printf("adopted children of subreaper (unreaped):\n"); fflush(stdout); system(cmd);
 int nz=0; while(waitpid(-1,0,WNOHANG)>0) nz++; printf("reaped %d adopted zombies\n",nz);
 /* subreaper survives exec? */
 if(fork()==0){ prctl(PR_SET_CHILD_SUBREAPER,1,0,0,0); execl("/proc/self/exe","x","chk",(char*)0); _exit(1);} wait(0);
 return 0;}
