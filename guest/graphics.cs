using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.IO;
using System.Runtime.InteropServices;
using System.Text;
using Microsoft.Win32.SafeHandles;

namespace Wdesk {
  // Fixed application-local software preset; Windows system DLLs are untouched.
  public static class Graphics {
    const string Cache="workspace/.wdesk/graphics/software-v1";
    static readonly string[] GL={"opengl32.dll","libgallium_wgl.dll"};
    static readonly string[] GLES={"libEGL.dll","libGLESv1_CM.dll","libGLESv2.dll"};
    [DllImport("ntdll.dll")]static extern int NtSetInformationFile(SafeFileHandle file,out IOStatus status,IntPtr info,uint length,int kind);
    [DllImport("ntdll.dll")]static extern uint RtlNtStatusToDosError(int status);
    [DllImport("kernel32.dll",SetLastError=true)]static extern bool GetFileInformationByHandle(SafeFileHandle file,IntPtr info);
    [StructLayout(LayoutKind.Sequential)]struct IOStatus {public IntPtr Status;public UIntPtr Information;}
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]struct LinkInfo {public byte Replace;public IntPtr Root;public uint Length;public char Name;}

    static void Architecture(string arch) {
      if(arch!="x64" && arch!="x86")throw new ArgumentException("Use x64 or x86");
    }
    public static string Machine(string executable) {
      using(var file=Native.OpenScoped(executable,FileMode.Open,FileAccess.Read))
      using(var read=new BinaryReader(file)) {
        if(file.Length<64 || read.ReadUInt16()!=0x5a4d)throw new ArgumentException("Expected a Windows PE executable");
        file.Position=60;uint offset=read.ReadUInt32();
        if(offset>file.Length-24)throw new ArgumentException("Invalid PE header");
        file.Position=offset;if(read.ReadUInt32()!=0x4550)throw new ArgumentException("Invalid PE signature");
        ushort machine=read.ReadUInt16();
        if(machine==0x8664)return "x64";
        if(machine==0x14c) {
          // AnyCPU executables are PE32 too; respect the CLR's bitness flags.
          ushort sections=read.ReadUInt16();file.Position=offset+20;ushort optional=read.ReadUInt16();
          if(optional>=216 && offset+24L+optional+40L*sections<=file.Length && sections<=96) {
            file.Position=offset+24L+208;uint clr=read.ReadUInt32();
            if(clr!=0)for(int i=0;i<sections;i++) {
              file.Position=offset+24L+optional+40L*i+12;
              uint rva=read.ReadUInt32(),size=read.ReadUInt32(),raw=read.ReadUInt32();
              if(clr>=rva && (long)clr-rva+20<=size) {
                long at=(long)raw+clr-rva+16;if(at+4>file.Length)throw new ArgumentException("Invalid CLR header");
                file.Position=at;uint flags=read.ReadUInt32();
                if((flags&1)!=0 && (flags&(2|0x20000))==0)return Environment.Is64BitOperatingSystem?"x64":"x86";
                break;
              }
            }
          }
          return "x86";
        }
        throw new ArgumentException("Unsupported PE architecture");
      }
    }
    public static string[] Components(string arch,string[] apis) {
      Architecture(arch);
      if(apis==null || apis.Length==0 || apis.Length>3)throw new ArgumentException("Select gl, gles, vk");
      var names=new List<string>();
      foreach(string api in apis) {
        if(api=="gl" || api=="gles")foreach(string name in GL)if(!names.Contains(name))names.Add(name);
        if(api=="gles")foreach(string name in GLES)if(!names.Contains(name))names.Add(name);
        if(api=="vk")foreach(string name in new[]{"vulkan_lvp.dll","vulkan-1.dll",arch=="x64"?"lvp_icd.x86_64.json":"lvp_icd.x86.json"})if(!names.Contains(name))names.Add(name);
        if(api!="gl" && api!="gles" && api!="vk")throw new ArgumentException("Select gl, gles, vk");
      }
      return names.ToArray();
    }
    public static object Target(string executable) {
      return new {architecture=Machine(executable),directory=executable.Substring(0,executable.LastIndexOf('/'))};
    }
    public static object Status(string arch,string[] apis) {
      var files=new List<object>();bool installed=true;long bytes=0;
      foreach(string name in Components(arch,apis)) {
        string remote=Cache+"/"+arch+"/"+name,path=Native.ScopedPath(remote,false);
        bool present=File.Exists(path);long size=present?new FileInfo(path).Length:0;
        files.Add(new {name=name,path=remote,present=present,bytes=size});installed&=present;bytes+=size;
      }
      return new {preset="software-v1",architecture=arch,installed=installed,bytes=bytes,files=files,render_verified=false};
    }
    static void Link(string source,string destination) {
      string dst=Native.ScopedPath(destination,false);
      if(File.Exists(dst))throw new IOException("Destination DLL exists; prepare a clean application directory: "+destination);
      using(var parent=TransferNative.Parent(destination))
      using(var handle=TransferNative.LinkSource(source)) {
        byte[] name=Encoding.Unicode.GetBytes(Path.GetFileName(dst));
        int at=(int)Marshal.OffsetOf(typeof(LinkInfo),"Name"),size=Marshal.SizeOf(typeof(LinkInfo))+name.Length;
        IntPtr info=Marshal.AllocHGlobal(size);
        try {
          for(int i=0;i<size;i++)Marshal.WriteByte(info,i,0);
          Marshal.WriteIntPtr(info,(int)Marshal.OffsetOf(typeof(LinkInfo),"Root"),parent.DangerousGetHandle());
          Marshal.WriteInt32(info,(int)Marshal.OffsetOf(typeof(LinkInfo),"Length"),name.Length);
          Marshal.Copy(name,0,IntPtr.Add(info,at),name.Length);
          IOStatus status;int result=NtSetInformationFile(handle,out status,info,(uint)size,11);
          if(result<0)throw new Win32Exception((int)RtlNtStatusToDosError(result));
        } finally {Marshal.FreeHGlobal(info);}
      }
    }
    public static object Prepare(string executable,string[] apis) {
      string arch=Machine(executable),directory=executable.Substring(0,executable.LastIndexOf('/'));
      string[] names=Components(arch,apis);
      // Repeat deployment reuses only the exact cached file, never another DLL.
      foreach(string name in names) {
        string source=Cache+"/"+arch+"/"+name,destination=directory+"/"+name;
        if(!File.Exists(Native.ScopedPath(source,false)))throw new FileNotFoundException("Install graphics runtime first: "+name);
        if(File.Exists(Native.ScopedPath(destination,false)) && !SameFile(source,destination))throw new IOException("Destination DLL exists: "+name);
      }
      var linked=new List<string>();
      try {
        foreach(string name in names) {
          string destination=directory+"/"+name;
          if(File.Exists(Native.ScopedPath(destination,false)))continue;
          Link(Cache+"/"+arch+"/"+name,destination);linked.Add(destination);
        }
      } catch {
        foreach(string path in linked)File.Delete(Native.ScopedPath(path,false));
        throw;
      }
      var deployed=new List<string>();foreach(string name in names)deployed.Add(directory+"/"+name);
      return new {preset="software-v1",architecture=arch,executable=executable,files=deployed,created=linked.Count,reused=names.Length-linked.Count,
        storage="NTFS hard links; one cached copy per architecture",environment=EnvironmentFor(arch,apis)};
    }
    public static Dictionary<string,string> EnvironmentFor(string arch,string[] apis) {
      Components(arch,apis);
      var env=new Dictionary<string,string>();
      env["GALLIUM_DRIVER"]="llvmpipe";env["LP_NUM_THREADS"]=Math.Min(Environment.ProcessorCount,8).ToString();
      foreach(string api in apis)if(api=="vk")env["VK_DRIVER_FILES"]=Native.ScopedPath(Cache+"/"+arch+"/"+(arch=="x64"?"lvp_icd.x86_64.json":"lvp_icd.x86.json"),false);
      return env;
    }
    static bool SameFile(string source,string destination) {
      using(var a=Native.OpenScoped(source,FileMode.Open,FileAccess.Read))
      using(var b=Native.OpenScoped(destination,FileMode.Open,FileAccess.Read)) {
        IntPtr left=Marshal.AllocHGlobal(52),right=Marshal.AllocHGlobal(52);
        try {
          if(!GetFileInformationByHandle(a.SafeFileHandle,left) || !GetFileInformationByHandle(b.SafeFileHandle,right))throw new Win32Exception();
          return Marshal.ReadInt32(left,28)==Marshal.ReadInt32(right,28) && Marshal.ReadInt64(left,44)==Marshal.ReadInt64(right,44);
        } finally {Marshal.FreeHGlobal(left);Marshal.FreeHGlobal(right);}
      }
    }
  }
}
