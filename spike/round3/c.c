#include "common.h"
static const char*LF="/private/tmp/claude-501/dk-review3/c.leaves";
static void addleaf(pid_t p){ FILE*f=fopen(LF,"a"); fprintf(f,"%d\n",p); fclose(f);}
int main(int argc,char**argv){ init();
 const char*m=argc>1?argv[1]:"";
 if(!strcmp(m,"leaf")){ alarm(8); addleaf(getpid()); sleep(6); return 0; }
 if(!strcmp(m,"dbl")){ alarm(8); pid_t c=fork(); if(c==0){ setsid(); if(fork()==0){ usleep(30000); execl(argv[0],argv[0],"leaf",(char*)0); _exit(1);} _exit(0);} waitpid(c,0,0);
   if(argc>2&&!strcmp(argv[2],"plat")){ pid_t c2=fork(); if(c2==0){ setsid(); if(fork()==0){ execl("/bin/sh","sh","-c","echo $$ >> /private/tmp/claude-501/dk-review3/c.leaves; exec /bin/sleep 6",(char*)0);} _exit(0);} waitpid(c2,0,0);} 
   usleep(200000); return 0; }
 if(!strcmp(m,"reexec")){ posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); posix_spawnattr_setflags(&a,POSIX_SPAWN_SETEXEC); char*v[]={argv[0],"dbl",NULL}; pid_t p; posix_spawn(&p,argv[0],NULL,&a,v,environ); return 1; }
 if(!strcmp(m,"sup")){ alarm(15); unlink(LF);
   uint64_t su=U(getpid()); printf("sup pid=%d resp(self)=%d\n",getpid(),resp(getpid()));
   pid_t r; posix_spawnattr_t a; posix_spawnattr_init(&a); if(posix_spawn(&r,argv[2],NULL,&a,argv+2,environ)){perror("spawn");return 1;}
   usleep(20000); printf("root pid=%d resp=%d\n",r,resp(r)); int st; waitpid(r,&st,0); usleep(300000);
   FILE*f=fopen(LF,"r"); int p; while(f&&fscanf(f,"%d",&p)==1){ printf("  leaf %d resp=%d  %s\n",p,resp(p),resp(p)==getpid()?"IN R":"NOT IN R"); }
   if(argc>0 && getenv("STOPTEST")){ printf("sup stopping itself\n"); fflush(stdout);
     pid_t q=fork(); if(q==0){ usleep(300000); FILE*f=fopen(LF,"r"); int p; while(fscanf(f,"%d",&p)==1) printf("  [sup STOPPED, state checked by helper] leaf %d resp=%d\n",p,resp(p)); fflush(stdout); kill(getppid(),SIGCONT); _exit(0);} 
     raise(SIGSTOP); waitpid(q,0,0); printf("sup continued\n"); }
   f=fopen(LF,"r"); rewind(f); while(f&&fscanf(f,"%d",&p)==1) kill(p,9); return 0; }
 /* launcher: self-disclaim into sup */
 posix_spawnattr_t a; posix_spawnattr_init(&a); disc(&a,1); posix_spawnattr_setflags(&a,POSIX_SPAWN_SETEXEC);
 char**v=calloc(argc+2,sizeof*v); v[0]=argv[0]; v[1]="sup"; for(int i=1;i<argc;i++) v[i+1]=argv[i]; pid_t p; posix_spawn(&p,argv[0],NULL,&a,v,environ); return 1; }
