//! Log thread-local: encoder e demais módulos chamam `emit(...)` em vez de
//! `println!`. A GUI configura um Sender no início de cada spawn pra capturar
//! as mensagens e exibir num painel próprio.
//!
//! Em modo console (CLI ou debug), ainda imprime no stdout pra facilitar
//! diagnóstico via terminal.

use std::cell::RefCell;
use std::sync::mpsc::Sender;

thread_local! {
    static LOG_TX: RefCell<Option<Sender<String>>> = const { RefCell::new(None) };
}

/// Configura o sender para a thread atual. Passe `None` pra resetar.
pub fn set_sender(tx: Option<Sender<String>>) {
    LOG_TX.with(|cell| *cell.borrow_mut() = tx);
}

/// Emite uma mensagem de log: imprime no stdout e envia pro sender da thread (se houver).
pub fn emit(msg: impl Into<String>) {
    let s = msg.into();
    println!("{s}");
    LOG_TX.with(|cell| {
        if let Some(tx) = cell.borrow().as_ref() {
            let _ = tx.send(s);
        }
    });
}
