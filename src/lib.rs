use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::{self, JoinHandle};

pub trait Actress<Msg: Send + 'static>: Send {
    fn process(&mut self, _m: Msg, _sys: &System<Msg>);
}

pub struct ActressDeletedError<Msg>(pub Address, pub Msg);

#[derive(Debug, Clone, Copy, Hash)]
pub struct Address(usize);

pub struct System<Msg: Send + 'static> {
    actresses: Arc<RwLock<Vec<Option<Box<(Mutex<Box<dyn Actress<Msg>>>, Mutex<VecDeque<Msg>>)>>>>>,
    handles: UnsafeCell<Option<Vec<JoinHandle<()>>>>,
    join: Arc<AtomicBool>,
    should_wait: Arc<AtomicBool>,
}

impl<Msg: Send + 'static> System<Msg> {
    pub fn new(num_threads: usize) -> Arc<Self> {
        let actresses = Arc::new(RwLock::new(Vec::new()));
        let join = Arc::new(AtomicBool::new(false));
        let should_wait = Arc::new(AtomicBool::new(false));

        let mut handles = Vec::new();

        let s = Arc::new(Self {
            actresses: actresses.clone(),
            handles: UnsafeCell::new(None),
            join: join.clone(),
            should_wait: should_wait.clone(),
        });

        for i in 0..num_threads {
            let join = join.clone();
            let should_wait = should_wait.clone();
            let actresses = actresses.clone();
            let s = s.clone();

            handles.push(thread::spawn(move || {
                while !join.load(Ordering::Acquire) {
                    for _ in 0..actresses.read().unwrap().len() {
                        while should_wait.load(Ordering::Acquire) {
                            std::hint::spin_loop()
                        }

                        let (actress, msg): &(Mutex<Box<dyn Actress<Msg>>>, Mutex<VecDeque<Msg>>) =
                            if let Some(x) = &actresses.read().unwrap()[i] {
                                unsafe { &*(&**x as *const _) }
                            } else {
                                continue;
                            };

                        if let Ok(mut act) = actress.try_lock() {
                            let msg = if let Some(msg) = msg.lock().unwrap().pop_front() {
                                msg
                            } else {
                                continue;
                            };

                            act.process(msg, &s);
                        }
                    }
                }
            }));
        }

        unsafe {
            *s.handles.get() = Some(handles);
        }

        s
    }

    pub fn create(&self, actress: Box<dyn Actress<Msg>>) -> Address {
        self.should_wait.store(true, Ordering::Release);

        let mut w = self.actresses.write().unwrap();

        w.push(Some(Box::new((
            Mutex::new(actress),
            Mutex::new(VecDeque::new()),
        ))));

        self.should_wait.store(false, Ordering::Release);

        Address(w.len() - 1)
    }

    pub fn send(&self, msg: Msg, add: Address) -> Result<(), ActressDeletedError<Msg>> {
        if let Some(x) = &self.actresses.read().unwrap()[add.0] {
            x.1.lock().unwrap().push_back(msg);

            Ok(())
        } else {
            Err(ActressDeletedError(add, msg))
        }
    }

    pub fn remove(&self, add: Address) {
        self.should_wait.store(true, Ordering::Release);

        let mut w = self.actresses.write().unwrap();

        w[add.0] = None;

        self.should_wait.store(false, Ordering::Release);
    }

    fn stop(&self) {
        self.join.store(false, Ordering::Release);
    }
}

impl<Msg: Send + 'static> Drop for System<Msg> {
    fn drop(&mut self) {
        self.stop();

        for i in unsafe { (*self.handles.get()).take().unwrap() } {
            i.join().unwrap();
        }
    }
}

unsafe impl<Msg: Send + 'static> Sync for System<Msg> {}
