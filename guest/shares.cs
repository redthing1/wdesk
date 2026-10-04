using System;
using System.Collections.Generic;
using System.ComponentModel;
using System.Runtime.InteropServices;
using System.Text;
using System.Text.RegularExpressions;

namespace Wdesk {
  // Only the trusted runtime calls this; it is not a data-plane guest operation.
  public static class Shares {
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]
    struct Resource { public uint Scope,Type,Display,Usage;public string Local,Remote,Comment,Provider; }
    [StructLayout(LayoutKind.Sequential,CharSet=CharSet.Unicode)]
    struct Credential {
      public uint Flags,Type;public string Target,Comment;public long LastWritten;
      public uint BlobSize;public IntPtr Blob;public uint Persist,Attributes;public IntPtr Attribute;
      public string Alias,User;
    }
    [DllImport("mpr.dll",CharSet=CharSet.Unicode)] static extern uint WNetAddConnection2(ref Resource resource,string password,string user,uint flags);
    [DllImport("mpr.dll",CharSet=CharSet.Unicode)] static extern uint WNetCancelConnection2(string name,uint flags,bool force);
    [DllImport("advapi32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool CredWrite(ref Credential credential,uint flags);
    static readonly List<string> Connected=new List<string>();

    public static object Attach(string address,string user,string password,string[] names) {
      if(address!="10.0.2.102" || user!="wdesk" || password==null || !Regex.IsMatch(password,"^[a-f0-9]{64}$") || names==null || names.Length<1 || names.Length>8)
        throw new ArgumentException("Invalid owner share attachment");
      foreach(string name in names)if(!Regex.IsMatch(name,"^[A-Za-z0-9_-]{1,48}$"))throw new ArgumentException("Invalid share name");
      foreach(string old in Connected)WNetCancelConnection2(old,0,true);
      Connected.Clear();
      // User credential vault, not process arguments or plaintext helper state.
      // Persistent user credentials can also be resolved by a linked UAC token.
      byte[] bytes=Encoding.Unicode.GetBytes(password);IntPtr blob=Marshal.AllocHGlobal(bytes.Length);
      try {
        Marshal.Copy(bytes,0,blob,bytes.Length);
        var cred=new Credential{Type=2,Target=address,BlobSize=(uint)bytes.Length,Blob=blob,Persist=2,User=user};
        if(!CredWrite(ref cred,0))throw new Win32Exception(Marshal.GetLastWin32Error());
      } finally {
        for(int i=0;i<bytes.Length;i++)Marshal.WriteByte(blob,i,0);
        Marshal.FreeHGlobal(blob);Array.Clear(bytes,0,bytes.Length);
      }
      foreach(string name in names) {
        string remote="\\\\"+address+"\\"+name;
        var resource=new Resource{Type=1,Remote=remote};
        uint result=WNetAddConnection2(ref resource,password,user,0);
        if(result!=0)throw new Win32Exception((int)result);
        Connected.Add(remote);
      }
      return new {attached=true,paths=Connected.ToArray()};
    }
  }
}
