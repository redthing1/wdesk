using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Diagnostics;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using System.Threading;
using System.Threading.Tasks;
using Microsoft.Win32.SafeHandles;

namespace Wdesk {
  public class GuestWire : IDisposable {
    System.IO.Ports.SerialPort serial;
    System.Net.Sockets.TcpClient fast;
    Task<System.Net.Sockets.TcpClient> connecting;
    DateTime retry=DateTime.MinValue;
    StringBuilder serialBuffer=new StringBuilder(),fastBuffer=new StringBuilder();
    Decoder decoder=Encoding.UTF8.GetDecoder();
    byte[] bytes=new byte[65536];char[] chars=new char[Encoding.UTF8.GetMaxCharCount(65536)];
    public string LastTransport {get;private set;}
    public GuestWire(){
      serial=new System.IO.Ports.SerialPort("COM1",115200,System.IO.Ports.Parity.None,8,System.IO.Ports.StopBits.One);
      serial.Encoding=Encoding.UTF8;serial.NewLine="\n";serial.ReadTimeout=1000;serial.WriteTimeout=15000;
      serial.ReadBufferSize=262144;serial.WriteBufferSize=262144;serial.Open();
    }
    void Connect(){
      if(fast!=null)return;
      if(connecting!=null&&connecting.IsCompleted){
        if(connecting.Status==TaskStatus.RanToCompletion){fast=connecting.Result;fast.NoDelay=true;fast.GetStream().WriteTimeout=15000;}
        else{var ignored=connecting.Exception;retry=DateTime.UtcNow.AddSeconds(5);}
        connecting=null;
      }
      if(fast==null&&connecting==null&&DateTime.UtcNow>=retry){
        connecting=Task.Run(delegate{
          var client=new System.Net.Sockets.TcpClient();
          try{client.Connect("10.0.2.100",9843);return client;}catch{client.Dispose();throw;}
        });
      }
    }
    void Disconnect(){if(fast!=null)fast.Dispose();fast=null;fastBuffer.Clear();decoder.Reset();retry=DateTime.UtcNow.AddSeconds(2);}
    string Line(StringBuilder buffer,string transport){
      if(buffer.Length>262144){buffer.Clear();throw new InvalidDataException("Oversized guest request");}
      string text=buffer.ToString();int n=text.IndexOf('\n');if(n<0)return null;
      string line=text.Substring(0,n);buffer.Remove(0,n+1);LastTransport=transport;return line;
    }
    public string ReadRequest(){
      Connect();serialBuffer.Append(serial.ReadExisting());
      string line=Line(serialBuffer,"serial");if(line!=null)return line;
      return ReadFast();
    }
    string ReadFast(){
      if(fast!=null){
        try{
          string queued=Line(fastBuffer,"tcp_guestfwd");if(queued!=null)return queued;
          // Readability means data OR EOF. An Available==0 check before Poll
          // races arriving data; only a zero-byte read proves disconnection.
          if(fast.Client.Poll(0,System.Net.Sockets.SelectMode.SelectRead)){
            int n=fast.GetStream().Read(bytes,0,bytes.Length);if(n==0){Disconnect();return null;}
            int count=decoder.GetChars(bytes,0,n,chars,0);fastBuffer.Append(chars,0,count);
          }
          return Line(fastBuffer,"tcp_guestfwd");
        }catch(IOException){Disconnect();return null;}catch(System.Net.Sockets.SocketException){Disconnect();return null;}
      }
      return null;
    }
    public void Respond(string line){
      if(LastTransport=="tcp_guestfwd"){
        try{byte[] output=Encoding.UTF8.GetBytes(line+"\n");fast.GetStream().Write(output,0,output.Length);}
        catch{Disconnect();throw;}
      }else serial.WriteLine(line);
    }
    public void Dispose(){serial.Dispose();Disconnect();if(connecting!=null&&connecting.Status==TaskStatus.RanToCompletion)connecting.Result.Dispose();}
  }

  public static class Native {
    [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
    public delegate bool EnumProc(IntPtr h,IntPtr p);
    [DllImport("user32.dll")] static extern bool EnumWindows(EnumProc callback,IntPtr p);
    [DllImport("user32.dll")] static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll",CharSet=CharSet.Unicode)] static extern int GetWindowText(IntPtr h,StringBuilder text,int count);
    [DllImport("user32.dll")] static extern bool GetWindowRect(IntPtr h,out Rect r);
    [DllImport("user32.dll")] static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
    [DllImport("user32.dll")] static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool IsWindow(IntPtr h);
    [DllImport("user32.dll")] static extern bool ShowWindow(IntPtr h,int command);
    [DllImport("user32.dll")] static extern bool SetProcessDPIAware();
    [DllImport("user32.dll",SetLastError=true)] static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
    [DllImport("user32.dll")] static extern bool CloseDesktop(IntPtr h);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern uint GetFinalPathNameByHandle(SafeFileHandle h,StringBuilder path,uint size,uint flags);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern uint SearchPath(string path,string file,string extension,int size,StringBuilder buffer,IntPtr filePart);
    [DllImport("user32.dll",SetLastError=true)] static extern uint SendInput(uint count,Input[] inputs,int size);
    [StructLayout(LayoutKind.Sequential)] struct Input {public uint Type; public InputUnion Data;}
    [StructLayout(LayoutKind.Explicit)] struct InputUnion { [FieldOffset(0)] public MouseInput Mouse;[FieldOffset(0)] public KeyboardInput Key; }
    [StructLayout(LayoutKind.Sequential)] struct MouseInput {public int X,Y;public uint Data,Flags,Time;public UIntPtr Extra;}
    [StructLayout(LayoutKind.Sequential)] struct KeyboardInput {public ushort Vk,Scan;public uint Flags,Time;public UIntPtr Extra;}
    public static void Init(){SetProcessDPIAware();}
    public static bool Interactive(){IntPtr h=OpenInputDesktop(0,false,0x0001);if(h==IntPtr.Zero)return false;CloseDesktop(h);return Process.GetCurrentProcess().SessionId!=0;}
    public static object[] Windows(){
      var result=new List<object>();
      EnumWindows(delegate(IntPtr h,IntPtr p){
        if(!IsWindowVisible(h))return true;
        var title=new StringBuilder(4096);GetWindowText(h,title,title.Capacity);if(title.Length==0)return true;
        Rect r;GetWindowRect(h,out r);uint pid;GetWindowThreadProcessId(h,out pid);
        result.Add(new {id=h.ToInt64().ToString(),pid=pid,title=title.ToString(),focused=h==GetForegroundWindow(),bounds=new {x=r.Left,y=r.Top,width=r.Right-r.Left,height=r.Bottom-r.Top}});return result.Count<1024;
      },IntPtr.Zero);return result.ToArray();
    }
    public static bool Focus(string id){long n;if(!Int64.TryParse(id,out n))throw new ArgumentException("Invalid window id");IntPtr h=new IntPtr(n);if(!IsWindow(h))throw new ArgumentException("Window no longer exists");ShowWindow(h,9);SetForegroundWindow(h);Thread.Sleep(100);return GetForegroundWindow()==h;}
    public static void TypeText(string text){
      if(text==null||text.Length>16384||text.IndexOf('\0')>=0)throw new ArgumentException("Invalid text");
      for(int index=0;index<text.Length;index++){
        char c=text[index];
        if(c=='\r'&&index+1<text.Length&&text[index+1]=='\n')continue;
        bool control=c=='\r'||c=='\n'||c=='\t';ushort vk=(ushort)(c=='\t'?9:13);
        var inputs=new Input[]{new Input{Type=1,Data=new InputUnion{Key=control?new KeyboardInput{Vk=vk}:new KeyboardInput{Scan=c,Flags=4}}},new Input{Type=1,Data=new InputUnion{Key=control?new KeyboardInput{Vk=vk,Flags=2}:new KeyboardInput{Scan=c,Flags=6}}}};
        if(SendInput(2,inputs,Marshal.SizeOf(typeof(Input)))!=2)throw new InvalidOperationException("SendInput failed; desktop may be locked or target elevated");
      }
    }
    public static string Quote(string s){
      if(s==null||s.IndexOf('\0')>=0)throw new ArgumentException("Invalid argument");
      if(s.Length>0&&s.IndexOfAny(new char[]{' ','\t','\n','\r','"'})<0)return s;
      var b=new StringBuilder("\"");int slashes=0;
      foreach(char c in s){if(c=='\\'){slashes++;continue;}if(c=='\"'){b.Append('\\',slashes*2+1);b.Append(c);}else{b.Append('\\',slashes);b.Append(c);}slashes=0;}
      b.Append('\\',slashes*2);return b.Append('"').ToString();
    }
    public static string Executable(string name){
      if(name==null||name.IndexOf('\0')>=0)throw new ArgumentException("Invalid executable");
      if(Path.IsPathRooted(name)){if(!File.Exists(name))throw new FileNotFoundException(name);return Path.GetFullPath(name);}
      var buffer=new StringBuilder(32768);uint n=SearchPath(null,name,".exe",buffer.Capacity,buffer,IntPtr.Zero);
      if(n==0||n>=buffer.Capacity)throw new FileNotFoundException("Executable not found: "+name);return buffer.ToString();
    }
    public static string ScopedPath(string remote,bool directory){
      if(String.IsNullOrEmpty(remote)||remote.Length>240||remote.Contains("\\")||remote.Contains(":"))throw new ArgumentException("Use workspace/ or downloads/ paths with forward slashes");
      string[] parts=remote.Split('/');if(parts[0]!="workspace"&&parts[0]!="downloads")throw new ArgumentException("Path outside configured roots");
      string root="C:\\ProgramData\\wdesk\\"+parts[0];string path=root;
      for(int index=1;index<parts.Length;index++){
        string part=parts[index];
        if(part==""||part=="."||part==".."||part.EndsWith(".")||part.EndsWith(" ")||part.IndexOfAny(Path.GetInvalidFileNameChars())>=0)throw new ArgumentException("Invalid path component");
        string stem=part.Split('.')[0].ToUpperInvariant();
        if(stem=="CON"||stem=="PRN"||stem=="AUX"||stem=="NUL"||System.Text.RegularExpressions.Regex.IsMatch(stem,"^(COM|LPT)[0-9]+$"))throw new ArgumentException("Device name not allowed");
        path=Path.Combine(path,part);
      }
      string probe=path;
      while(probe!=null&&probe.StartsWith(root,StringComparison.OrdinalIgnoreCase)){
        if((File.Exists(probe)||Directory.Exists(probe))&&(File.GetAttributes(probe)&FileAttributes.ReparsePoint)!=0)throw new ArgumentException("Reparse points are not allowed");
        probe=Path.GetDirectoryName(probe);
      }
      if(!directory&&path==root)throw new ArgumentException("A file path is required");
      return path;
    }
    public static FileStream OpenScoped(string remote,FileMode mode,FileAccess access){
      string path=ScopedPath(remote,false);
      string root="C:\\ProgramData\\wdesk\\"+remote.Split('/')[0]+"\\";
      if(access!=FileAccess.Read)Directory.CreateDirectory(Path.GetDirectoryName(path));
      var stream=new FileStream(path,mode,access,FileShare.Read,65536,FileOptions.SequentialScan);
      try{var final=new StringBuilder(32768);uint n=GetFinalPathNameByHandle(stream.SafeFileHandle,final,(uint)final.Capacity,0);
        if(n==0||n>=final.Capacity||!final.ToString().StartsWith("\\\\?\\"+root,StringComparison.OrdinalIgnoreCase))throw new ArgumentException("Opened target escapes configured root");
        return stream;
      }catch{stream.Dispose();throw;}
    }
  }

  public static class TransferNative {
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
    static extern SafeFileHandle CreateFile(string path,uint access,uint share,IntPtr security,uint mode,uint flags,IntPtr template);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)]
    static extern uint GetFinalPathNameByHandle(SafeFileHandle file,StringBuilder path,uint length,uint flags);
    [DllImport("kernel32.dll",SetLastError=true)]
    static extern bool SetFileInformationByHandle(SafeFileHandle file,int kind,IntPtr info,uint length);
    [DllImport("ntdll.dll")]
    static extern int NtSetInformationFile(SafeFileHandle file,out IOStatus status,IntPtr info,uint length,int kind);
    [DllImport("ntdll.dll")]
    static extern uint RtlNtStatusToDosError(int status);
    [StructLayout(LayoutKind.Sequential)] struct IOStatus {public IntPtr Status;public UIntPtr Information;}
    [DllImport("kernel32.dll",SetLastError=true)]
    static extern bool GetFileInformationByHandleEx(SafeFileHandle file,int kind,out AttributeInfo info,uint length);
    [StructLayout(LayoutKind.Sequential)] struct AttributeInfo {public uint Attributes,Tag;}
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]
    struct RenameInfo {public byte Replace;public IntPtr Root;public uint Length;public char Name;}
    static void Verify(SafeFileHandle handle,string root,bool directory) {
      var final=new StringBuilder(32768);uint n=GetFinalPathNameByHandle(handle,final,(uint)final.Capacity,0);
      string expected="\\\\?\\"+root.TrimEnd('\\');
      AttributeInfo attributes;
      if(!GetFileInformationByHandleEx(handle,9,out attributes,8))throw new Win32Exception();
      if((attributes.Attributes&0x400)!=0)throw new ArgumentException("Reparse points are not allowed");
      if(n==0 || n>=final.Capacity || !(final.ToString().StartsWith(expected+"\\",StringComparison.OrdinalIgnoreCase) ||
          (directory && final.ToString().Equals(expected,StringComparison.OrdinalIgnoreCase))))
        throw new ArgumentException("Opened target escapes configured root");
    }
    public static SafeFileHandle Parent(string remote) {
      string path=Native.ScopedPath(remote,false),parent=Path.GetDirectoryName(path);
      Directory.CreateDirectory(parent);Native.ScopedPath(remote,false);
      var handle=CreateFile(parent,0xA0,3,IntPtr.Zero,3,0x02200000,IntPtr.Zero);
      try {
        if(handle.IsInvalid)throw new Win32Exception();
        Verify(handle,"C:\\ProgramData\\wdesk\\"+remote.Split('/')[0],true);
        return handle;
      } catch {handle.Dispose();throw;}
    }
    public static SafeFileHandle LinkSource(string remote) {
      string path=Native.ScopedPath(remote,false);
      var handle=CreateFile(path,0x80010000,1,IntPtr.Zero,3,0x00200000,IntPtr.Zero);
      try {
        if(handle.IsInvalid)throw new Win32Exception();
        Verify(handle,"C:\\ProgramData\\wdesk\\"+remote.Split('/')[0],false);
        return handle;
      } catch {handle.Dispose();throw;}
    }
    public static FileStream Stage(string path) {
      string root="C:\\ProgramData\\wdesk\\staging";
      if(!path.StartsWith(root+"\\",StringComparison.OrdinalIgnoreCase) ||
          (File.GetAttributes(root)&FileAttributes.ReparsePoint)!=0)throw new ArgumentException("Invalid staging root");
      var handle=CreateFile(path,0xC0010000,1,IntPtr.Zero,1,0x08000000,IntPtr.Zero);
      try {
        if(handle.IsInvalid)throw new Win32Exception();
        Verify(handle,root,false);
        return new FileStream(handle,FileAccess.ReadWrite,262144,false);
      } catch {
        if(!handle.IsInvalid) {
          var disposition=Marshal.AllocHGlobal(1);
          try {Marshal.WriteByte(disposition,1);SetFileInformationByHandle(handle,4,disposition,1);}
          finally {Marshal.FreeHGlobal(disposition);}
        }
        handle.Dispose();throw;
      }
    }
    public static void Promote(FileStream stream,SafeFileHandle parent,string remote) {
      string path=Native.ScopedPath(remote,false);
      Verify(parent,"C:\\ProgramData\\wdesk\\"+remote.Split('/')[0],true);
      byte[] name=Encoding.Unicode.GetBytes(Path.GetFileName(path));
      int at=(int)Marshal.OffsetOf(typeof(RenameInfo),"Name"),size=Marshal.SizeOf(typeof(RenameInfo))+name.Length;
      IntPtr info=Marshal.AllocHGlobal(size);
      try {
        for(int i=0;i<size;i++)Marshal.WriteByte(info,i,0);
        Marshal.WriteByte(info,0,1);
        Marshal.WriteIntPtr(info,(int)Marshal.OffsetOf(typeof(RenameInfo),"Root"),parent.DangerousGetHandle());
        Marshal.WriteInt32(info,(int)Marshal.OffsetOf(typeof(RenameInfo),"Length"),name.Length);
        Marshal.Copy(name,0,IntPtr.Add(info,at),name.Length);
        // The Win32 wrapper rejects non-null RootDirectory on tested Windows.
        // Native rename keeps resolution anchored to the verified parent handle.
        IOStatus status;
        int result=NtSetInformationFile(stream.SafeFileHandle,out status,info,(uint)size,10);
        if(result<0)throw new Win32Exception((int)RtlNtStatusToDosError(result));
      } finally {Marshal.FreeHGlobal(info);}
    }
  }

  public class OwnedProcess : IDisposable {
    [StructLayout(LayoutKind.Sequential)] struct Security {public int Length;public IntPtr Descriptor;[MarshalAs(UnmanagedType.Bool)]public bool Inherit;}
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)] struct Startup {public int cb;public string Reserved,Desktop,Title;public uint X,Y,XSize,YSize,XChars,YChars,Fill,Flags;public ushort Show,Reserved2;public IntPtr ReservedPtr,Input,Output,Error;}
    [StructLayout(LayoutKind.Sequential)] struct ProcessInfo {public IntPtr Process,Thread;public uint Pid,Tid;}
    [StructLayout(LayoutKind.Sequential)] struct JobBasic {public long ProcessTime,JobTime;public uint Flags;public UIntPtr Min,Max;public uint Active;public UIntPtr Affinity;public uint Priority,Scheduling;}
    [StructLayout(LayoutKind.Sequential)] struct IoCounters {public ulong ReadOps,WriteOps,OtherOps,ReadBytes,WriteBytes,OtherBytes;}
    [StructLayout(LayoutKind.Sequential)] struct JobExtended {public JobBasic Basic;public IoCounters Io;public UIntPtr ProcessMemory,JobMemory,PeakProcessMemory,PeakJobMemory;}
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool CreatePipe(out IntPtr read,out IntPtr write,ref Security security,uint size);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetHandleInformation(IntPtr h,uint mask,uint flags);
    [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CreateProcess(string app,StringBuilder line,IntPtr pa,IntPtr ta,bool inherit,uint flags,IntPtr env,string cwd,ref Startup startup,out ProcessInfo info);
    [DllImport("kernel32.dll",SetLastError=true)] static extern IntPtr CreateJobObject(IntPtr security,string name);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool SetInformationJobObject(IntPtr job,int kind,ref JobExtended info,uint size);
    [DllImport("kernel32.dll",SetLastError=true)] static extern bool AssignProcessToJobObject(IntPtr job,IntPtr process);
    [DllImport("kernel32.dll")] static extern bool TerminateJobObject(IntPtr job,uint exit);
    [DllImport("kernel32.dll")] static extern bool TerminateProcess(IntPtr process,uint exit);
    [DllImport("kernel32.dll")] static extern uint ResumeThread(IntPtr thread);
    [DllImport("kernel32.dll")] static extern uint WaitForSingleObject(IntPtr process,uint milliseconds);
    [DllImport("kernel32.dll")] static extern bool GetExitCodeProcess(IntPtr process,out uint exit);
    [DllImport("kernel32.dll")] static extern bool CloseHandle(IntPtr h);
    IntPtr process,job;Timer timer;Task reader;StringBuilder output=new StringBuilder();object gate=new object(),life=new object();bool truncated=false,timedOut=false,disposed=false;int outputLimit;object exitCode;
    public uint Pid {get;private set;}
    public bool Running {get{lock(life)return process!=IntPtr.Zero&&WaitForSingleObject(process,0)==258;}}
    public bool TimedOut {get{lock(life)return timedOut;}}
    public bool Truncated {get{lock(gate)return truncated;}}
    public object ExitCode {get{lock(life){uint code;if(Running)return null;if(process!=IntPtr.Zero&&GetExitCodeProcess(process,out code))exitCode=code;return exitCode;}}}
    public string Output {get{lock(gate)return output.ToString();}}
    public bool OutputComplete {get{return reader!=null&&reader.Status==TaskStatus.RanToCompletion;}}
    public OwnedProcess(string[] argv,string cwd,int seconds):this(argv,cwd,seconds,65536){}
    public OwnedProcess(string[] argv,string cwd,int seconds,int retainCharacters):this(argv,cwd,seconds,retainCharacters,null){}
    public OwnedProcess(string[] argv,string cwd,int seconds,int retainCharacters,System.Collections.Generic.Dictionary<string,string> environment){
      if(retainCharacters<1||retainCharacters>1048576)throw new ArgumentException("Invalid output limit");outputLimit=retainCharacters;
      if(argv==null||argv.Length==0||argv.Length>128||seconds<1||seconds>3600)throw new ArgumentException("Invalid process options");
      var line=new StringBuilder();foreach(string arg in argv){if(line.Length>0)line.Append(' ');line.Append(Native.Quote(arg));}if(line.Length>30000)throw new ArgumentException("Command line too long");
      IntPtr read=IntPtr.Zero,write=IntPtr.Zero,env=IntPtr.Zero;ProcessInfo info=new ProcessInfo();
      try{
        if(environment!=null) {
          var values=new System.Collections.Generic.SortedDictionary<string,string>(StringComparer.OrdinalIgnoreCase);
          foreach(System.Collections.DictionaryEntry entry in Environment.GetEnvironmentVariables())values[(string)entry.Key]=(string)entry.Value;
          foreach(var entry in environment)values[entry.Key]=entry.Value;
          var block=new StringBuilder();foreach(var entry in values)block.Append(entry.Key).Append('=').Append(entry.Value).Append('\0');block.Append('\0');
          env=Marshal.StringToHGlobalUni(block.ToString());
        }
        job=CreateJobObject(IntPtr.Zero,null);if(job==IntPtr.Zero)throw new Win32Exception();
        var limits=new JobExtended();limits.Basic.Flags=0x2000;
        if(!SetInformationJobObject(job,9,ref limits,(uint)Marshal.SizeOf(typeof(JobExtended))))throw new Win32Exception();
        var security=new Security{Length=Marshal.SizeOf(typeof(Security)),Inherit=true};
        if(!CreatePipe(out read,out write,ref security,0)||!SetHandleInformation(read,1,0))throw new Win32Exception();
        var start=new Startup{cb=Marshal.SizeOf(typeof(Startup)),Flags=0x100,Output=write,Error=write,Input=IntPtr.Zero};
        if(!CreateProcess(Native.Executable(argv[0]),line,IntPtr.Zero,IntPtr.Zero,true,4|0x08000000|0x400,env,cwd,ref start,out info))throw new Win32Exception();
        process=info.Process;Pid=info.Pid;
        if(!AssignProcessToJobObject(job,process))throw new Win32Exception();
        var handle=new SafeFileHandle(read,true);read=IntPtr.Zero;
        reader=Task.Run(delegate{
          using(var stream=new FileStream(handle,FileAccess.Read))using(var text=new StreamReader(stream,Encoding.UTF8,true,4096)){
            var buffer=new char[4096];int count;while((count=text.Read(buffer,0,buffer.Length))>0){lock(gate){int remaining=outputLimit-output.Length;int n=Math.Min(count,remaining);if(n>0)output.Append(buffer,0,n);if(n<count)truncated=true;}}
          }
        });
        timer=new Timer(delegate{lock(life){if(Running){timedOut=true;Kill();}}},null,seconds*1000,Timeout.Infinite);
        if(ResumeThread(info.Thread)==UInt32.MaxValue)throw new Win32Exception();
      }catch{if(process!=IntPtr.Zero)TerminateProcess(process,1);Dispose();throw;}
      finally{if(env!=IntPtr.Zero)Marshal.FreeHGlobal(env);if(read!=IntPtr.Zero)CloseHandle(read);if(write!=IntPtr.Zero)CloseHandle(write);if(info.Thread!=IntPtr.Zero)CloseHandle(info.Thread);}
    }
    public void Kill(){lock(life){if(job!=IntPtr.Zero)TerminateJobObject(job,1);}}
    // Parent exit is the end of an owned process tree, not permission for its
    // descendants to keep running or hold redirected pipes open indefinitely.
    public void FinishTree(){lock(life){if(!Running){exitCode=ExitCode;if(timer!=null)timer.Dispose();if(job!=IntPtr.Zero)CloseHandle(job);job=IntPtr.Zero;}}}
    public void Drain(){FinishTree();if(!Running&&reader!=null)reader.Wait(1000);}
    public void Dispose(){lock(life){if(disposed)return;disposed=true;exitCode=ExitCode;if(timer!=null)timer.Dispose();if(job!=IntPtr.Zero)CloseHandle(job);if(process!=IntPtr.Zero)CloseHandle(process);job=IntPtr.Zero;process=IntPtr.Zero;}}
  }

  // Handles are retired independently of desktop requests. Only bounded,
  // immutable completion receipts survive; ids expire on eviction or restart.
  public sealed class Processes : IDisposable {
    public const int ActiveLimit=128,ReceiptLimit=128,ReceiptSeconds=600;
    sealed class Entry {public OwnedProcess Process;public long Exited;}
    sealed class Receipt {public System.Collections.Generic.Dictionary<string,object> Result;public long Completed;public System.Collections.Generic.LinkedListNode<string> Node;}
    readonly object gate=new object();bool disposed;
    readonly System.Collections.Generic.Dictionary<string,Entry> active=new System.Collections.Generic.Dictionary<string,Entry>();
    readonly System.Collections.Generic.Dictionary<string,Receipt> completed=new System.Collections.Generic.Dictionary<string,Receipt>();
    readonly System.Collections.Generic.LinkedList<string> order=new System.Collections.Generic.LinkedList<string>();
    readonly Timer timer;
    static long Now {get{return System.Diagnostics.Stopwatch.GetTimestamp();}}
    static bool Older(long start,int seconds){return Now-start>=seconds*System.Diagnostics.Stopwatch.Frequency;}
    public Processes(){timer=new Timer(delegate{lock(gate){if(!disposed)Reap();}},null,250,250);}
    static System.Collections.Generic.Dictionary<string,object> Snapshot(string id,OwnedProcess p,string phase){
      return new System.Collections.Generic.Dictionary<string,object>{{"id",id},{"pid",p.Pid},{"running",p.Running},{"phase",phase},{"exit_code",p.ExitCode},{"output",p.Output},{"output_truncated",p.Truncated},{"output_complete",p.OutputComplete},{"timed_out",p.TimedOut}};
    }
    void Reap(){
      foreach(var pair in new System.Collections.Generic.List<System.Collections.Generic.KeyValuePair<string,Entry>>(active)){
        var entry=pair.Value;var p=entry.Process;if(p.Running)continue;
        p.FinishTree();if(entry.Exited==0)entry.Exited=Now;
        if(!p.OutputComplete&&!Older(entry.Exited,2))continue;
        var result=Snapshot(pair.Key,p,"completed");p.Dispose();active.Remove(pair.Key);
        completed[pair.Key]=new Receipt{Result=result,Completed=Now,Node=order.AddLast(pair.Key)};
      }
      while(order.Count>0){
        string id=order.First.Value;Receipt receipt=completed[id];
        if(completed.Count<=ReceiptLimit&&!Older(receipt.Completed,ReceiptSeconds))break;
        completed.Remove(id);order.RemoveFirst();
      }
    }
    public System.Collections.Generic.Dictionary<string,object> Start(string[] argv,string cwd,int seconds,System.Collections.Generic.Dictionary<string,string> environment){
      lock(gate){
        if(disposed)throw new ObjectDisposedException("Processes");Reap();
        if(active.Count>=ActiveLimit)throw new InvalidOperationException("Active process limit (128); wait for completion or kill an owned process");
        string id=Guid.NewGuid().ToString();var p=new OwnedProcess(argv,cwd,seconds,65536,environment);
        active[id]=new Entry{Process=p};return Snapshot(id,p,p.Running?"running":"draining");
      }
    }
    public System.Collections.Generic.Dictionary<string,object> Get(string id){
      lock(gate){Reap();Entry entry;Receipt receipt;
        if(active.TryGetValue(id,out entry))return Snapshot(id,entry.Process,entry.Process.Running?"running":"draining");
        if(completed.TryGetValue(id,out receipt))return new System.Collections.Generic.Dictionary<string,object>(receipt.Result);
        throw new InvalidOperationException("Unknown or expired process id; completions retain at most 128 receipts for 10 minutes in this helper incarnation");
      }
    }
    public System.Collections.Generic.Dictionary<string,object> Kill(string id){
      lock(gate){Entry entry;if(active.TryGetValue(id,out entry))entry.Process.Kill();return Get(id);}
    }
    public void Forget(string id){
      lock(gate){Reap();if(active.ContainsKey(id))throw new InvalidOperationException("Process is running or draining; wait or kill before forgetting");
        Receipt receipt;if(!completed.TryGetValue(id,out receipt))throw new InvalidOperationException("Unknown or expired process id");completed.Remove(id);order.Remove(receipt.Node);
      }
    }
    public void Dispose(){lock(gate){if(disposed)return;disposed=true;timer.Dispose();foreach(var entry in active.Values)entry.Process.Dispose();active.Clear();completed.Clear();order.Clear();}}
  }
}
