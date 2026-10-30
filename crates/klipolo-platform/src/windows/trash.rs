//! The Recycle Bin, for `folder::move_to_trash`: `IFileOperation` deleting with `FOFX_RECYCLEONDELETE` (and
//! `FOF_ALLOWUNDO`), with no UI, on a COM apartment of this thread's own for the call. Explorer can put it back.
//!
//! **Never a permanent delete.** With no UI, the Shell answers its own "too big for the Recycle
//! Bin; delete it permanently?" itself (`FOF_NO_UI` includes `FOF_NOCONFIRMATION`), and a volume without a Recycle Bin
//! deletes outright. So a progress sink refuses, in `PreDeleteItem`, every delete that wouldn't recycle
//! (`trash_rule::recycles`): the take stays where it is, and the error is `FileTooLarge`.

use std::io;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use windows::Win32::Foundation::E_ABORT;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT, COINIT_APARTMENTTHREADED, COINIT_DISABLE_OLE1DDE, CoCreateInstance,
    CoInitializeEx, CoUninitialize,
};
use windows::Win32::UI::Shell::{
    FILEOPERATION_FLAGS, FOF_ALLOWUNDO, FOF_NO_UI, FOFX_RECYCLEONDELETE, FileOperation,
    IFileOperation, IFileOperationProgressSink, IFileOperationProgressSink_Impl, IShellItem,
    SHCreateItemFromParsingName,
};
use windows::core::{HRESULT, HSTRING, PCWSTR, Ref, implement};

use crate::trash_rule::recycles;

/// Where it went isn't said: the Recycle Bin names its items itself (Start over's Undo is W's, through it).
pub(crate) fn move_to_trash(path: &Path) -> io::Result<Option<std::path::PathBuf>> {
    let path = std::path::absolute(path)?;
    let refused = Arc::new(AtomicBool::new(false));
    // SAFETY: COM is initialised for this call on this thread (S_FALSE: already was) and released after; every
    // interface is used only inside it.
    let result = unsafe {
        let init = CoInitializeEx(
            None,
            COINIT(COINIT_APARTMENTTHREADED.0 | COINIT_DISABLE_OLE1DDE.0),
        );
        let result = recycle(&path, refused.clone());
        if init.is_ok() {
            CoUninitialize();
        }
        result
    };
    if refused.load(Ordering::SeqCst) {
        return Err(io::Error::new(
            io::ErrorKind::FileTooLarge,
            "the Recycle Bin can't take it, so it was kept",
        ));
    }
    result.map(|()| None).map_err(io::Error::other)
}

/// Not yet: the Recycle Bin's own restore (W, with Start over's Undo there).
pub(crate) fn put_back(_trashed: &Path, _to: &Path) -> io::Result<()> {
    Err(io::ErrorKind::Unsupported.into())
}

unsafe fn recycle(path: &Path, refused: Arc<AtomicBool>) -> windows::core::Result<()> {
    unsafe {
        let op: IFileOperation = CoCreateInstance(&FileOperation, None, CLSCTX_ALL)?;
        op.SetOperationFlags(FILEOPERATION_FLAGS(
            FOF_NO_UI.0 | FOF_ALLOWUNDO.0 | FOFX_RECYCLEONDELETE.0,
        ))?;
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        let sink: IFileOperationProgressSink = RecycleOnly { refused }.into();
        op.DeleteItem(&item, &sink)?;
        op.PerformOperations()?;
        if op.GetAnyOperationsAborted()?.as_bool() {
            return Err(E_ABORT.into());
        }
        Ok(())
    }
}

/// Lets a delete through only when it recycles.
#[implement(IFileOperationProgressSink)]
struct RecycleOnly {
    refused: Arc<AtomicBool>,
}

type Done = windows::core::Result<()>;

impl IFileOperationProgressSink_Impl for RecycleOnly_Impl {
    fn PreDeleteItem(&self, dwflags: u32, _item: Ref<IShellItem>) -> Done {
        if recycles(dwflags) {
            return Ok(());
        }
        self.refused.store(true, Ordering::SeqCst);
        Err(E_ABORT.into())
    }

    fn StartOperations(&self) -> Done {
        Ok(())
    }
    fn FinishOperations(&self, _: HRESULT) -> Done {
        Ok(())
    }
    fn PreRenameItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Done {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> Done {
        Ok(())
    }
    fn PreMoveItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Done {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> Done {
        Ok(())
    }
    fn PreCopyItem(&self, _: u32, _: Ref<IShellItem>, _: Ref<IShellItem>, _: &PCWSTR) -> Done {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> Done {
        Ok(())
    }
    fn PostDeleteItem(&self, _: u32, _: Ref<IShellItem>, _: HRESULT, _: Ref<IShellItem>) -> Done {
        Ok(())
    }
    fn PreNewItem(&self, _: u32, _: Ref<IShellItem>, _: &PCWSTR) -> Done {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: Ref<IShellItem>,
        _: &PCWSTR,
        _: &PCWSTR,
        _: u32,
        _: HRESULT,
        _: Ref<IShellItem>,
    ) -> Done {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> Done {
        Ok(())
    }
    fn ResetTimer(&self) -> Done {
        Ok(())
    }
    fn PauseTimer(&self) -> Done {
        Ok(())
    }
    fn ResumeTimer(&self) -> Done {
        Ok(())
    }
}
