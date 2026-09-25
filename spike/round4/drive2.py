import os, pty, time, signal
pid, fd = pty.fork()
if pid == 0:
    os.execv("/bin/sh", ["sh","-c","./sig; true"])
time.sleep(0.5)
os.write(fd, b"\x03")          # tty-generated INT
time.sleep(0.4)
os.kill(pid, signal.SIGINT)    # kill() INT from parent
time.sleep(0.4)
os.close(fd)                   # hang up the terminal
time.sleep(0.8)
try: os.kill(pid, signal.SIGKILL)
except ProcessLookupError: pass
os.waitpid(pid, 0)
