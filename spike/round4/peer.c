#include "/Users/lukasoberhuber/projects/deepkill/spike/round3/common.h"
#include <sys/socket.h>
#include <sys/un.h>
#include <bsm/libbsm.h>
#ifndef LOCAL_PEERTOKEN
#define LOCAL_PEERTOKEN 0x006
#endif
static int32_t IDV(pid_t p){ struct uq u; return proc_pidinfo(p,17,0,&u,sizeof u)==sizeof u?u.a:-99; }
static void peer(int c,const char*t){ pid_t pp=0; socklen_t l=sizeof pp; int r=getsockopt(c,SOL_LOCAL,LOCAL_PEERPID,&pp,&l);
  audit_token_t at; socklen_t la=sizeof at; int r2=getsockopt(c,SOL_LOCAL,LOCAL_PEERTOKEN,&at,&la);
  printf("%-28s PEERPID r=%d pid=%d | PEERTOKEN r=%d pid=%d pidver=%d (live idversion of that pid=%d)\n",t,r,pp,r2,r2==0?audit_token_to_pid(at):-1,r2==0?audit_token_to_pidversion(at):-1, IDV(r2==0?audit_token_to_pid(at):pp)); fflush(stdout);}
static void sendfd(int s,int fd){ struct msghdr m={0}; char b[CMSG_SPACE(sizeof(int))]; char d='x'; struct iovec io={&d,1}; m.msg_iov=&io;m.msg_iovlen=1;m.msg_control=b;m.msg_controllen=sizeof b; struct cmsghdr*c=CMSG_FIRSTHDR(&m); c->cmsg_level=SOL_SOCKET;c->cmsg_type=SCM_RIGHTS;c->cmsg_len=CMSG_LEN(sizeof(int)); memcpy(CMSG_DATA(c),&fd,sizeof fd); sendmsg(s,&m,0);}
static int recvfd(int s){ struct msghdr m={0}; char b[CMSG_SPACE(sizeof(int))]; char d; struct iovec io={&d,1}; m.msg_iov=&io;m.msg_iovlen=1;m.msg_control=b;m.msg_controllen=sizeof b; if(recvmsg(s,&m,0)<=0) return -1; int fd; memcpy(&fd,CMSG_DATA(CMSG_FIRSTHDR(&m)),sizeof fd); return fd;}
int main(){ init(); char path[]="/private/tmp/claude-501/dk-review4/s.sock"; unlink(path);
 int L=socket(AF_UNIX,SOCK_STREAM,0); struct sockaddr_un a={0}; a.sun_family=AF_UNIX; strcpy(a.sun_path,path); bind(L,(void*)&a,sizeof a); listen(L,8);
 int sp[2]; socketpair(AF_UNIX,SOCK_STREAM,0,sp);
 pid_t B=fork(); if(!B){ close(sp[0]); int fd=recvfd(sp[1]); usleep(300000); write(fd,"B",1); usleep(500000); _exit(0);}  /* B: receives fd, writes later */
 pid_t A=fork(); if(!A){ int c=socket(AF_UNIX,SOCK_STREAM,0); connect(c,(void*)&a,sizeof a); usleep(100000); sendfd(sp[0],c); close(c); _exit(0);} /* A: connects, passes fd, exits */
 printf("A=%d B=%d\n",A,B);
 int c=accept(L,0,0); peer(c,"after accept (A alive)");
 waitpid(A,0,0); peer(c,"A exited+reaped, B holds fd");
 char x; read(c,&x,1); peer(c,"after B wrote");
 waitpid(B,0,0);
 /* case 2: connect then exec (same pid) */
 pid_t E=fork(); if(!E){ int c2=socket(AF_UNIX,SOCK_STREAM,0); connect(c2,(void*)&a,sizeof a); usleep(100000); execl("/bin/sleep","sleep","0.5",(char*)0); _exit(1);}
 int c3=accept(L,0,0); peer(c3,"E before exec"); usleep(300000); peer(c3,"E after exec(/bin/sleep)"); waitpid(E,0,0); peer(c3,"E reaped");
 /* case 3: connect then fork, parent exits, child writes */
 pid_t F=fork(); if(!F){ int c4=socket(AF_UNIX,SOCK_STREAM,0); connect(c4,(void*)&a,sizeof a); pid_t g=fork(); if(!g){ usleep(200000); write(c4,"G",1); usleep(300000); _exit(0);} printf("F=%d G=%d\n",getpid(),g); fflush(stdout); _exit(0);}
 int c5=accept(L,0,0); waitpid(F,0,0); peer(c5,"F exited, G holds"); read(c5,&x,1); peer(c5,"after G wrote"); usleep(400000);
 unlink(path); return 0; }
