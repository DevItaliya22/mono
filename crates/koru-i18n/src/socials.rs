// SPDX-License-Identifier: MPL-2.0

//! Social scheduling, composing and account setup in all eight languages.
//!
//! Each row has exactly eight strings in [`Lang::ALL`] order. The macro keeps
//! the keys and rows together, so a new key cannot lack a translation or index
//! another key's row. Lookup does not allocate or fall back to English.
//! These are plain labels and complete messages, with no interpolation slots.
//! Translations require native-speaker review before release, like the main
//! catalog.

use crate::Lang;

macro_rules! catalog {
    ($($key:ident => $row:expr),+ $(,)?) => {
        /// A Socials label or message, available in every supported language.
        #[derive(Clone, Copy, Debug, Eq, PartialEq)]
        #[repr(usize)]
        pub enum Key { $($key),+ }

        impl Key {
            /// All keys in catalog order.
            pub const ALL: &'static [Self] = &[$(Self::$key),+];
        }

        const ROWS: &[[&str; 8]] = &[$($row),+];
    };
}

// English, Spanish, Brazilian Portuguese, German, French, Arabic,
// Simplified Chinese, Japanese. The array length makes omissions fail to compile.
catalog! {
    LinkedInOAuthHint => ["Use a LinkedIn app with native PKCE enabled and permission to publish. Reconnect when its access expires.", "Usa una aplicación de LinkedIn con PKCE nativo activado y permiso para publicar. Vuelve a conectar cuando caduque el acceso.", "Use um aplicativo do LinkedIn com PKCE nativo ativado e permissão para publicar. Reconecte quando o acesso expirar.", "Verwende eine LinkedIn-App mit aktiviertem nativem PKCE und Berechtigung zum Veröffentlichen. Verbinde sie erneut, wenn der Zugriff abläuft.", "Utilisez une application LinkedIn avec PKCE natif activé et l’autorisation de publier. Reconnectez-la à l’expiration de l’accès.", "استخدم تطبيق LinkedIn مع تفعيل PKCE للتطبيقات الأصلية وصلاحية النشر. أعد الربط عند انتهاء صلاحية الوصول.", "请使用已启用原生 PKCE 且具有发布权限的 LinkedIn 应用。访问权限到期后，请重新连接。", "ネイティブ PKCE が有効で投稿権限のある LinkedIn アプリを使用してください。アクセスの有効期限が切れたら再連携してください。"],
    RedditOAuthHint => ["Use an approved Reddit installed app. Grant permission to identify your account and submit posts.", "Usa una aplicación instalada aprobada por Reddit. Concede permiso para identificar tu cuenta y enviar publicaciones.", "Use um aplicativo instalado aprovado pelo Reddit. Conceda permissão para identificar sua conta e enviar publicações.", "Verwende eine genehmigte installierte Reddit-App. Erteile die Berechtigung, dein Konto zu identifizieren und Beiträge zu veröffentlichen.", "Utilisez une application installée approuvée par Reddit. Autorisez l’identification de votre compte et l’envoi de publications.", "استخدم تطبيق Reddit مثبتًا ومعتمدًا. امنحه صلاحية تحديد حسابك وإرسال المنشورات.", "请使用经 Reddit 批准的已安装应用，并授予识别账号和提交帖子的权限。", "Reddit に承認されたインストール型アプリを使用してください。アカウントの識別と投稿の送信を許可してください。"],
    LinkedInPageHint => ["Enter the numeric LinkedIn Page ID. Your account must have permission to publish for that Page.", "Introduce el ID numérico de la página de LinkedIn. Tu cuenta debe tener permiso para publicar en esa página.", "Insira o ID numérico da página do LinkedIn. Sua conta deve ter permissão para publicar nessa página.", "Gib die numerische ID der LinkedIn-Seite ein. Dein Konto muss auf dieser Seite veröffentlichen dürfen.", "Saisissez l’ID numérique de la page LinkedIn. Votre compte doit avoir l’autorisation de publier sur cette page.", "أدخل المعرّف الرقمي لصفحة LinkedIn. يجب أن يملك حسابك صلاحية النشر لهذه الصفحة.", "请输入 LinkedIn 主页的数字 ID。你的账号必须具有该主页的发布权限。", "LinkedIn ページの数値 ID を入力してください。アカウントにはそのページへの投稿権限が必要です。"],
    OrganizationId => ["Page ID", "ID de página", "ID da página", "Seiten-ID", "ID de page", "معرّف الصفحة", "主页 ID", "ページ ID"],
    OrganizationRequired => ["Enter a valid numeric LinkedIn Page ID.", "Introduce un ID numérico válido de página de LinkedIn.", "Insira um ID numérico válido de página do LinkedIn.", "Gib eine gültige numerische LinkedIn-Seiten-ID ein.", "Saisissez un ID numérique de page LinkedIn valide.", "أدخل معرّفًا رقميًا صالحًا لصفحة LinkedIn.", "请输入有效的 LinkedIn 主页数字 ID。", "有効な LinkedIn ページの数値 ID を入力してください。"],
    PublishingThisDevice => ["Publishes from this device. Keep it awake and online.", "Se publica desde este dispositivo. Mantenlo activo y conectado.", "As publicações são enviadas deste dispositivo. Mantenha-o ativo e conectado.", "Veröffentlicht von diesem Gerät. Lass es eingeschaltet und online.", "Les publications sont envoyées depuis cet appareil. Gardez-le éveillé et connecté.", "يتم النشر من هذا الجهاز. أبقه نشطًا ومتصلًا بالإنترنت.", "从此设备发布。请保持设备唤醒并联网。", "このデバイスから公開します。スリープさせず、オンラインにしておいてください。"],
    PublishingOtherDevice => ["Publishing is assigned to another device.", "La publicación está asignada a otro dispositivo.", "A publicação está atribuída a outro dispositivo.", "Die Veröffentlichung ist einem anderen Gerät zugewiesen.", "La publication est attribuée à un autre appareil.", "النشر مسند إلى جهاز آخر.", "发布任务已分配给其他设备。", "公開は別のデバイスに割り当てられています。"],
    NeedsDevice => ["Saved only — no publishing device", "Solo guardado — sin dispositivo de publicación", "Apenas salvo — sem dispositivo para publicação", "Nur gespeichert — kein Gerät zur Veröffentlichung", "Enregistré uniquement — aucun appareil de publication", "محفوظ فقط — لا يوجد جهاز للنشر", "仅保存 — 未分配发布设备", "保存のみ — 公開するデバイスがありません"],
    LegacySchedule => ["Copy this older post before scheduling it.", "Copia esta publicación antigua antes de programarla.", "Copie esta publicação antiga antes de agendá-la.", "Kopiere diesen älteren Beitrag, bevor du ihn planst.", "Copiez cette ancienne publication avant de la programmer.", "انسخ هذا المنشور القديم قبل جدولته.", "请先复制此旧帖子，再设置排期。", "この古い投稿をコピーしてから予約してください。"],
    Duplicate => ["Create a copy", "Crear una copia", "Criar uma cópia", "Kopie erstellen", "Créer une copie", "إنشاء نسخة", "创建副本", "コピーを作成"],
    RetryDelivery => ["Retry failed delivery", "Reintentar el envío fallido", "Tentar novamente o envio que falhou", "Fehlgeschlagenen Versand wiederholen", "Réessayer l’envoi échoué", "إعادة محاولة الإرسال الفاشل", "重试失败的发送", "失敗した送信を再試行"],
    RetryQueued => ["Retry requested", "Reintento solicitado", "Nova tentativa solicitada", "Wiederholung angefordert", "Nouvelle tentative demandée", "تم طلب إعادة المحاولة", "已请求重试", "再試行をリクエストしました"],
    CancelDelivery => ["Cancel remaining posts", "Cancelar publicaciones restantes", "Cancelar publicações restantes", "Verbleibende Beiträge abbrechen", "Annuler les publications restantes", "إلغاء المنشورات المتبقية", "取消剩余帖子", "残りの投稿をキャンセル"],
    CancelRequested => ["Cancellation requested", "Cancelación solicitada", "Cancelamento solicitado", "Abbruch angefordert", "Annulation demandée", "تم طلب الإلغاء", "已请求取消", "キャンセルをリクエストしました"],
    Cancelled => ["Cancelled", "Cancelado", "Cancelado", "Abgebrochen", "Annulé", "ملغى", "已取消", "キャンセル済み"],
    Waiting => ["Waiting to retry", "Esperando para reintentar", "Aguardando nova tentativa", "Wartet auf Wiederholung", "En attente d’une nouvelle tentative", "في انتظار إعادة المحاولة", "等待重试", "再試行待ち"],
    UnconfirmedHint => ["The service may have received this part. Check your account before creating another post.", "Es posible que el servicio haya recibido esta parte. Revisa tu cuenta antes de crear otra publicación.", "O serviço pode ter recebido esta parte. Confira sua conta antes de criar outra publicação.", "Der Dienst hat diesen Teil möglicherweise erhalten. Prüfe dein Konto, bevor du einen weiteren Beitrag erstellst.", "Le service a peut-être reçu cette partie. Vérifiez votre compte avant de créer une autre publication.", "ربما تلقّت الخدمة هذا الجزء. تحقق من حسابك قبل إنشاء منشور آخر.", "服务可能已收到这一部分。请先检查你的账号，再创建其他帖子。", "サービスがこの部分を受信した可能性があります。別の投稿を作成する前にアカウントを確認してください。"],
    OpenPublished => ["Open published post", "Ver publicación", "Ver publicação", "Veröffentlichten Beitrag öffnen", "Voir la publication", "عرض المنشور بعد نشره", "打开已发布的帖子", "公開済みの投稿を開く"],
    Channels => ["Channels", "Canales", "Canais", "Kanäle", "Canaux", "القنوات", "频道", "チャンネル"],
    Feed => ["Feed", "Noticias", "Feed", "Feed", "Fil d’actualité", "آخر المنشورات", "动态", "フィード"],
    Inbox => ["Inbox", "Bandeja de entrada", "Caixa de entrada", "Posteingang", "Boîte de réception", "الوارد", "收件箱", "受信トレイ"],
    Queue => ["Queue", "Cola", "Fila", "Warteschlange", "File d’attente", "قائمة الانتظار", "队列", "投稿待ち"],
    Posts => ["Posts", "Publicaciones", "Publicações", "Beiträge", "Publications", "المنشورات", "帖子", "投稿"],
    OpenSchedule => ["Open in schedule", "Abrir en la programación", "Abrir no calendário", "Im Zeitplan öffnen", "Ouvrir dans le calendrier", "فتح في الجدول", "在排期中打开", "スケジュールで開く"],
    AddToQueue => ["Add to queue", "Añadir a la cola", "Adicionar à fila", "Zur Warteschlange hinzufügen", "Ajouter à la file d’attente", "إضافة إلى قائمة الانتظار", "加入队列", "投稿待ちに追加"],
    ChannelQueueHint => ["Upcoming posts on this channel, soonest first", "Próximas publicaciones en este canal, las más cercanas primero", "Próximas publicações neste canal, começando pelas mais próximas", "Anstehende Beiträge auf diesem Kanal, die nächsten zuerst", "Prochaines publications sur ce canal, les plus proches en premier", "المنشورات القادمة على هذه القناة، الأقرب أولًا", "此频道即将发布的帖子，按时间从近到远排列", "このチャンネルの公開予定の投稿（予定が近い順）"],
    NoChannelPosts => ["No published posts on this channel yet.", "Aún no hay publicaciones en este canal.", "Nenhuma publicação neste canal ainda.", "Auf diesem Kanal wurden noch keine Beiträge veröffentlicht.", "Aucune publication sur ce canal pour le moment.", "لا توجد منشورات منشورة على هذه القناة بعد.", "此频道尚未发布任何帖子。", "このチャンネルにはまだ公開済みの投稿がありません。"],
    FeedInboxLater => ["Feed and inbox are coming later.", "Las noticias y la bandeja de entrada estarán disponibles más adelante.", "O feed e a caixa de entrada estarão disponíveis mais adiante.", "Feed und Posteingang folgen später.", "Le fil d’actualité et la boîte de réception seront disponibles plus tard.", "ستتوفر آخر المنشورات والوارد لاحقًا.", "动态和收件箱功能将于之后推出。", "フィードと受信トレイは今後追加される予定です。"],
    ConnectX => ["Connect X account", "Conectar cuenta de X", "Conectar conta do X", "X-Konto verbinden", "Connecter un compte X", "ربط حساب X", "连接 X 账号", "X アカウントを連携"],
    OAuthHint => ["Enter the public Client ID from your X Native App. Your browser will ask you to authorize this account.", "Introduce el ID de cliente público de tu aplicación nativa de X. El navegador te pedirá que autorices esta cuenta.", "Insira o ID de cliente público do seu aplicativo nativo do X. O navegador solicitará autorização para esta conta.", "Gib die öffentliche Client-ID deiner nativen X-App ein. Dein Browser wird dich um die Autorisierung dieses Kontos bitten.", "Saisissez l’ID client public de votre application native X. Votre navigateur vous demandera d’autoriser ce compte.", "أدخل معرّف العميل العام من تطبيق X الأصلي لديك. سيطلب منك المتصفح تفويض هذا الحساب.", "输入你的 X 原生应用的公开客户端 ID。浏览器将提示你授权此账号。", "X のネイティブアプリの公開クライアント ID を入力してください。ブラウザーでこのアカウントの認可を求められます。"],
    OAuthSuccess => ["Authorization received. Return to Koru to finish connecting.", "Autorización recibida. Vuelve a Koru para terminar de conectar la cuenta.", "Autorização recebida. Volte ao Koru para concluir a conexão.", "Autorisierung erhalten. Kehre zu Koru zurück, um die Verbindung abzuschließen.", "Autorisation reçue. Revenez à Koru pour terminer la connexion.", "تم استلام التفويض. عُد إلى Koru لإكمال الربط.", "已收到授权。请返回 Koru 完成连接。", "認可を受け取りました。Koru に戻って連携を完了してください。"],
    OAuthFailed => ["Could not connect this account. Try again.", "No se pudo conectar esta cuenta. Inténtalo de nuevo.", "Não foi possível conectar esta conta. Tente novamente.", "Dieses Konto konnte nicht verbunden werden. Versuche es erneut.", "Impossible de connecter ce compte. Réessayez.", "تعذر ربط هذا الحساب. أعد المحاولة.", "无法连接此账号。请重试。", "このアカウントを連携できませんでした。再試行してください。"],
    OAuthCancelled => ["Connection cancelled.", "Conexión cancelada.", "Conexão cancelada.", "Verbindung abgebrochen.", "Connexion annulée.", "تم إلغاء الربط.", "连接已取消。", "連携をキャンセルしました。"],
    OAuthBrowserHint => ["Finish authorizing in your browser. You can cancel here.", "Completa la autorización en el navegador. Puedes cancelarla aquí.", "Conclua a autorização no navegador. Você pode cancelar aqui.", "Schließe die Autorisierung in deinem Browser ab. Du kannst hier abbrechen.", "Terminez l’autorisation dans votre navigateur. Vous pouvez annuler ici.", "أكمل التفويض في المتصفح. يمكنك الإلغاء هنا.", "请在浏览器中完成授权。你可以在此取消。", "ブラウザーで認可を完了してください。ここでキャンセルできます。"],
    OAuthClientRequired => ["Enter your app’s Client ID.", "Introduce el ID de cliente de tu aplicación.", "Insira o ID de cliente do seu aplicativo.", "Gib die Client-ID deiner App ein.", "Saisissez l’ID client de votre application.", "أدخل معرّف العميل لتطبيقك.", "请输入你的应用的客户端 ID。", "アプリのクライアント ID を入力してください。"],
    OAuthCallbackBusy => ["The callback address is in use. Close another connection attempt and retry.", "La dirección de retorno está en uso. Cierra otro intento de conexión y vuelve a intentarlo.", "O endereço de retorno está em uso. Feche outra tentativa de conexão e tente novamente.", "Die Rückrufadresse wird bereits verwendet. Beende einen anderen Verbindungsversuch und versuche es erneut.", "L’adresse de retour est déjà utilisée. Fermez une autre tentative de connexion et réessayez.", "عنوان الاستدعاء قيد الاستخدام. أغلق محاولة ربط أخرى ثم أعد المحاولة.", "回调地址正在使用中。请关闭另一次连接尝试后重试。", "コールバックアドレスは使用中です。別の接続処理を閉じてから再試行してください。"],
    OAuthVaultFailed => ["Koru could not save the connection securely. The account was not connected.", "Koru no pudo guardar la conexión de forma segura. La cuenta no se ha conectado.", "O Koru não conseguiu salvar a conexão com segurança. A conta não foi conectada.", "Koru konnte die Verbindung nicht sicher speichern. Das Konto wurde nicht verbunden.", "Koru n’a pas pu enregistrer la connexion de manière sécurisée. Le compte n’a pas été connecté.", "تعذر على Koru حفظ الاتصال بأمان. لم يتم ربط الحساب.", "Koru 无法安全保存连接。账号未连接。", "Koru は連携情報を安全に保存できませんでした。アカウントは連携されていません。"],
    OAuthDenied => ["Authorization was declined.", "Se rechazó la autorización.", "A autorização foi recusada.", "Die Autorisierung wurde abgelehnt.", "L’autorisation a été refusée.", "تم رفض التفويض.", "授权被拒绝。", "認可が拒否されました。"],
    OAuthTimedOut => ["Authorization timed out. Try again.", "La autorización agotó el tiempo de espera. Inténtalo de nuevo.", "O tempo para autorização expirou. Tente novamente.", "Die Autorisierung hat zu lange gedauert. Versuche es erneut.", "Le délai d’autorisation a expiré. Réessayez.", "انتهت مهلة التفويض. أعد المحاولة.", "授权超时。请重试。", "認可がタイムアウトしました。再試行してください。"],
    OAuthApiFailed => ["The service could not verify this account. Check your app access and retry.", "El servicio no pudo verificar esta cuenta. Comprueba el acceso de tu aplicación y vuelve a intentarlo.", "O serviço não conseguiu verificar esta conta. Confira o acesso do seu aplicativo e tente novamente.", "Der Dienst konnte dieses Konto nicht verifizieren. Prüfe die Zugriffsrechte deiner App und versuche es erneut.", "Le service n’a pas pu vérifier ce compte. Vérifiez les accès de votre application et réessayez.", "تعذر على الخدمة التحقق من هذا الحساب. تحقق من صلاحيات تطبيقك ثم أعد المحاولة.", "服务无法验证此账号。请检查应用访问权限后重试。", "サービスでこのアカウントを確認できませんでした。アプリのアクセス権を確認して再試行してください。"],
    LocalCredentialsMissing => ["Reconnect this account on this device.", "Vuelve a conectar esta cuenta en este dispositivo.", "Reconecte esta conta neste dispositivo.", "Verbinde dieses Konto auf diesem Gerät erneut.", "Reconnectez ce compte sur cet appareil.", "أعد ربط هذا الحساب على هذا الجهاز.", "请在此设备上重新连接此账号。", "このデバイスでアカウントを再連携してください。"],
    Connected => ["Connected on this device", "Conectada en este dispositivo", "Conectada neste dispositivo", "Auf diesem Gerät verbunden", "Connecté sur cet appareil", "مرتبط على هذا الجهاز", "已在此设备上连接", "このデバイスで連携済み"],
    PublishingNotAvailable => ["Connecting an account does not schedule your drafts. Keep the publishing device awake and online.", "Conectar una cuenta no programa tus borradores. Mantén activo y conectado el dispositivo que publica.", "Conectar uma conta não agenda seus rascunhos. Mantenha o dispositivo de publicação ativo e conectado.", "Das Verbinden eines Kontos plant keine Entwürfe ein. Halte das Gerät zum Veröffentlichen wach und online.", "Connecter un compte ne programme pas vos brouillons. Gardez l’appareil de publication éveillé et connecté.", "ربط الحساب لا يجدول مسوداتك. أبقِ جهاز النشر نشطًا ومتصلًا بالإنترنت.", "连接账号不会为草稿设置排期。请保持发布设备唤醒并联网。", "アカウントを連携しても下書きは予約されません。公開するデバイスをスリープさせず、オンラインにしておいてください。"],
    List => ["List", "Lista", "Lista", "Liste", "Liste", "قائمة", "列表", "リスト"],
    Calendar => ["Calendar", "Calendario", "Calendário", "Kalender", "Calendrier", "تقويم", "日历", "カレンダー"],
    Title => ["Socials", "Redes sociales", "Redes sociais", "Social Media", "Réseaux sociaux", "التواصل الاجتماعي", "社交媒体", "ソーシャル"],
    Scheduled => ["Scheduled", "Programadas", "Agendadas", "Geplant", "Programmées", "مجدولة", "已排期", "予約済み"],
    Drafts => ["Drafts", "Borradores", "Rascunhos", "Entwürfe", "Brouillons", "المسودات", "草稿", "下書き"],
    History => ["History", "Historial", "Histórico", "Verlauf", "Historique", "السجل", "历史记录", "履歴"],
    Accounts => ["Accounts", "Cuentas", "Contas", "Konten", "Comptes", "الحسابات", "账号", "アカウント"],
    NewPost => ["New post", "Nueva publicación", "Nova publicação", "Neuer Beitrag", "Nouvelle publication", "منشور جديد", "新建帖子", "新規投稿"],
    SaveDraft => ["Save draft", "Guardar borrador", "Salvar rascunho", "Entwurf speichern", "Enregistrer le brouillon", "حفظ المسودة", "保存草稿", "下書きを保存"],
    Schedule => ["Schedule", "Programar", "Agendar", "Planen", "Programmer", "جدولة", "设置排期", "予約する"],
    Update => ["Update", "Actualizar", "Atualizar", "Aktualisieren", "Mettre à jour", "تحديث", "更新", "更新"],
    Delete => ["Delete", "Eliminar", "Excluir", "Löschen", "Supprimer", "حذف", "删除", "削除"],
    Back => ["Back", "Volver", "Voltar", "Zurück", "Retour", "رجوع", "返回", "戻る"],
    Close => ["Close", "Cerrar", "Fechar", "Schließen", "Fermer", "إغلاق", "关闭", "閉じる"],
    Refresh => ["Refresh", "Actualizar", "Atualizar", "Neu laden", "Actualiser", "تحديث", "刷新", "再読み込み"],
    Loading => ["Loading…", "Cargando…", "Carregando…", "Wird geladen…", "Chargement…", "جارٍ التحميل…", "正在加载…", "読み込み中…"],
    SaveFailed => ["Could not save. Your changes are still here.", "No se pudo guardar. Tus cambios siguen aquí.", "Não foi possível salvar. Suas alterações continuam aqui.", "Speichern fehlgeschlagen. Deine Änderungen sind noch hier.", "Enregistrement impossible. Vos modifications sont toujours ici.", "تعذر الحفظ. ما زالت تغييراتك هنا.", "无法保存。你的更改仍保留在此处。", "保存できませんでした。変更内容はここに保持されています。"],
    Saved => ["Saved", "Guardado", "Salvo", "Gespeichert", "Enregistré", "تم الحفظ", "已保存", "保存済み"],
    Saving => ["Saving…", "Guardando…", "Salvando…", "Wird gespeichert…", "Enregistrement…", "جارٍ الحفظ…", "正在保存…", "保存中…"],
    Unsaved => ["Unsaved changes", "Cambios sin guardar", "Alterações não salvas", "Ungespeicherte Änderungen", "Modifications non enregistrées", "تغييرات غير محفوظة", "有未保存的更改", "未保存の変更"],
    EmptySchedule => ["No scheduled posts yet.", "Aún no hay publicaciones programadas.", "Nenhuma publicação agendada ainda.", "Noch keine geplanten Beiträge.", "Aucune publication programmée pour le moment.", "لا توجد منشورات مجدولة بعد.", "还没有排期的帖子。", "予約済みの投稿はまだありません。"],
    EmptyDrafts => ["No drafts yet. Create your first post.", "Aún no hay borradores. Crea tu primera publicación.", "Nenhum rascunho ainda. Crie sua primeira publicação.", "Noch keine Entwürfe. Erstelle deinen ersten Beitrag.", "Aucun brouillon pour le moment. Créez votre première publication.", "لا توجد مسودات بعد. أنشئ منشورك الأول.", "还没有草稿。创建你的第一篇帖子。", "下書きはまだありません。最初の投稿を作成しましょう。"],
    EmptyHistory => ["No publication history yet.", "Aún no hay historial de publicaciones.", "Nenhum histórico de publicação ainda.", "Noch kein Veröffentlichungsverlauf.", "Aucun historique de publication pour le moment.", "لا يوجد سجل نشر بعد.", "还没有发布记录。", "公開履歴はまだありません。"],
    PostTitle => ["Post title", "Título de la publicación", "Título da publicação", "Beitragstitel", "Titre de la publication", "عنوان المنشور", "帖子标题", "投稿タイトル"],
    BodyPlaceholder => ["What would you like to share?", "¿Qué te gustaría compartir?", "O que você gostaria de compartilhar?", "Was möchtest du teilen?", "Que souhaitez-vous partager ?", "ماذا تود أن تشارك؟", "你想分享什么？", "何をシェアしますか？"],
    PerNetwork => ["Customize for each network", "Personalizar para cada red", "Personalizar para cada rede", "Für jedes Netzwerk anpassen", "Adapter à chaque réseau", "تخصيص لكل شبكة", "为各个平台定制", "各サービス向けに調整"],
    FollowsOriginal => ["Follows original", "Usa el original", "Usa o original", "Übernimmt das Original", "Suit l’original", "يتبع النص الأصلي", "跟随原文", "元の内容を使用"],
    Edited => ["Customized", "Personalizado", "Personalizado", "Angepasst", "Personnalisé", "مخصص", "已定制", "調整済み"],
    Edit => ["Edit", "Editar", "Editar", "Bearbeiten", "Modifier", "تعديل", "编辑", "編集"],
    Reset => ["Use original", "Usar original", "Usar original", "Original verwenden", "Utiliser l’original", "استخدام النص الأصلي", "使用原文", "元の内容に戻す"],
    NoChannels => ["Add an account to choose where to publish.", "Añade una cuenta para elegir dónde publicar.", "Adicione uma conta para escolher onde publicar.", "Füge ein Konto hinzu, um das Veröffentlichungsziel zu wählen.", "Ajoutez un compte pour choisir où publier.", "أضف حسابًا لاختيار مكان النشر.", "添加账号以选择发布位置。", "アカウントを追加して投稿先を選んでください。"],
    ConnectAccount => ["Connect account", "Conectar cuenta", "Conectar conta", "Konto verbinden", "Connecter un compte", "ربط حساب", "连接账号", "アカウントを連携"],
    AddChannel => ["Add account", "Añadir cuenta", "Adicionar conta", "Konto hinzufügen", "Ajouter un compte", "إضافة حساب", "添加账号", "アカウントを追加"],
    ChannelName => ["Account name", "Nombre de la cuenta", "Nome da conta", "Kontoname", "Nom du compte", "اسم الحساب", "账号名称", "アカウント名"],
    NotConnected => ["Not connected", "Sin conexión", "Não conectada", "Nicht verbunden", "Non connecté", "غير مرتبط", "未连接", "未連携"],
    NeedsConnection => ["Connect this account before publishing.", "Conecta esta cuenta antes de publicar.", "Conecte esta conta antes de publicar.", "Verbinde dieses Konto vor dem Veröffentlichen.", "Connectez ce compte avant de publier.", "اربط هذا الحساب قبل النشر.", "请先连接此账号再发布。", "公開する前にこのアカウントを連携してください。"],
    Draft => ["Draft", "Borrador", "Rascunho", "Entwurf", "Brouillon", "مسودة", "草稿", "下書き"],
    Queued => ["Queued", "En cola", "Na fila", "In Warteschlange", "En attente", "في قائمة الانتظار", "排队中", "待機中"],
    Publishing => ["Publishing", "Publicando", "Publicando", "Wird veröffentlicht", "Publication en cours", "جارٍ النشر", "正在发布", "公開中"],
    Published => ["Published", "Publicada", "Publicada", "Veröffentlicht", "Publiée", "تم النشر", "已发布", "公開済み"],
    Partial => ["Partially published", "Publicación parcial", "Publicação parcial", "Teilweise veröffentlicht", "Partiellement publiée", "نُشر جزئيًا", "部分已发布", "一部公開済み"],
    Failed => ["Failed", "Error", "Falhou", "Fehlgeschlagen", "Échec", "فشل", "失败", "失敗"],
    Unconfirmed => ["Check publication", "Comprobar publicación", "Verificar publicação", "Veröffentlichung prüfen", "Vérifier la publication", "تحقق من النشر", "请核实发布状态", "公開状況を確認"],
    Date => ["Date", "Fecha", "Data", "Datum", "Date", "التاريخ", "日期", "日付"],
    Time => ["Time", "Hora", "Hora", "Uhrzeit", "Heure", "الوقت", "时间", "時刻"],
    Today => ["Today", "Hoy", "Hoje", "Heute", "Aujourd’hui", "اليوم", "今天", "今日"],
    Previous => ["Previous", "Anterior", "Anterior", "Zurück", "Précédent", "السابق", "上一个", "前へ"],
    Next => ["Next", "Siguiente", "Próximo", "Weiter", "Suivant", "التالي", "下一个", "次へ"],
    Subreddit => ["Subreddit", "Subreddit", "Subreddit", "Subreddit", "Subreddit", "مجتمع Reddit", "Reddit 社区", "サブレディット"],
    RedditTitle => ["Reddit title", "Título en Reddit", "Título no Reddit", "Reddit-Titel", "Titre Reddit", "عنوان Reddit", "Reddit 标题", "Reddit のタイトル"],
    Flair => ["Post flair", "Etiqueta de publicación", "Etiqueta da publicação", "Beitrags-Flair", "Flair de publication", "وسم المنشور", "帖子标签", "投稿フレア"],
    FlairOptional => ["Flair (optional)", "Etiqueta (opcional)", "Etiqueta (opcional)", "Flair (optional)", "Flair (facultatif)", "الوسم (اختياري)", "标签（可选）", "フレア（任意）"],
    Person => ["Personal profile", "Perfil personal", "Perfil pessoal", "Persönliches Profil", "Profil personnel", "ملف شخصي", "个人资料", "個人プロフィール"],
    Page => ["Page", "Página", "Página", "Seite", "Page", "صفحة", "主页", "ページ"],
    NoDestination => ["Choose at least one account.", "Elige al menos una cuenta.", "Escolha pelo menos uma conta.", "Wähle mindestens ein Konto.", "Choisissez au moins un compte.", "اختر حسابًا واحدًا على الأقل.", "请至少选择一个账号。", "アカウントを1つ以上選んでください。"],
    EmptyBody => ["Write something before scheduling.", "Escribe algo antes de programar.", "Escreva algo antes de agendar.", "Schreibe etwas, bevor du den Beitrag planst.", "Écrivez quelque chose avant de programmer.", "اكتب شيئًا قبل الجدولة.", "请先填写内容再设置排期。", "予約する前に内容を入力してください。"],
    MissingSubreddit => ["Choose a subreddit.", "Elige un subreddit.", "Escolha um subreddit.", "Wähle ein Subreddit.", "Choisissez un subreddit.", "اختر مجتمعًا على Reddit.", "请选择 Reddit 社区。", "サブレディットを選んでください。"],
    MissingRedditTitle => ["Add a Reddit title.", "Añade un título para Reddit.", "Adicione um título para o Reddit.", "Füge einen Reddit-Titel hinzu.", "Ajoutez un titre pour Reddit.", "أضف عنوانًا لمنشور Reddit.", "请添加 Reddit 标题。", "Reddit のタイトルを入力してください。"],
    MissingFlair => ["This subreddit requires a post flair.", "Este subreddit requiere una etiqueta de publicación.", "Este subreddit exige uma etiqueta na publicação.", "Dieses Subreddit erfordert ein Beitrags-Flair.", "Ce subreddit exige un flair de publication.", "يتطلب هذا المجتمع وسمًا للمنشور.", "此社区要求帖子带有标签。", "このサブレディットでは投稿フレアが必要です。"],
    TooLong => ["This post exceeds the network’s length limit.", "Esta publicación supera el límite de longitud de la red.", "Esta publicação excede o limite de tamanho da rede.", "Dieser Beitrag überschreitet die Längenbegrenzung des Netzwerks.", "Cette publication dépasse la limite de longueur du réseau.", "يتجاوز هذا المنشور الحد الأقصى للطول في الشبكة.", "此帖子超出了平台的长度限制。", "この投稿はサービスの文字数制限を超えています。"],
    MissingTime => ["Choose a date and time.", "Elige una fecha y una hora.", "Escolha uma data e um horário.", "Wähle Datum und Uhrzeit.", "Choisissez une date et une heure.", "اختر التاريخ والوقت.", "请选择日期和时间。", "日付と時刻を選んでください。"],
    PastTime => ["Choose a time in the future.", "Elige una hora futura.", "Escolha um horário no futuro.", "Wähle einen Zeitpunkt in der Zukunft.", "Choisissez une date et une heure à venir.", "اختر وقتًا في المستقبل.", "请选择未来的时间。", "未来の日時を選んでください。"],
    ReadOnly => ["This post is read-only.", "Esta publicación es de solo lectura.", "Esta publicação é somente leitura.", "Dieser Beitrag ist schreibgeschützt.", "Cette publication est en lecture seule.", "هذا المنشور للقراءة فقط.", "此帖子为只读。", "この投稿は読み取り専用です。"],
    Source => ["Source document", "Documento de origen", "Documento de origem", "Quelldokument", "Document source", "المستند المصدر", "来源文档", "元のドキュメント"],
    SourceDetached => ["Source unavailable; your post is preserved.", "El origen no está disponible; tu publicación se conserva.", "A origem está indisponível; sua publicação está preservada.", "Quelle nicht verfügbar; dein Beitrag bleibt erhalten.", "Source indisponible ; votre publication est conservée.", "المصدر غير متاح؛ منشورك محفوظ.", "来源不可用；你的帖子仍然保留。", "元のドキュメントは利用できません。投稿内容は保持されています。"],
    OfflineHint => ["Scheduled posts publish when this device is awake and connected.", "Las publicaciones programadas se publican cuando este dispositivo está activo y conectado.", "As publicações agendadas são enviadas quando este dispositivo está ativo e conectado.", "Geplante Beiträge werden veröffentlicht, wenn dieses Gerät aktiv und online ist.", "Les publications programmées sont envoyées lorsque cet appareil est actif et connecté.", "تُنشر المنشورات المجدولة عندما يكون هذا الجهاز نشطًا ومتصلًا بالإنترنت.", "此设备处于唤醒状态且联网时，排期的帖子才会发布。", "予約投稿は、このデバイスが起動してインターネットに接続しているときに公開されます。"],
    SetupHint => ["Connect an account to schedule posts. Keep the publishing device awake and online.", "Conecta una cuenta para programar publicaciones. Mantén activo y conectado el dispositivo que publica.", "Conecte uma conta para agendar publicações. Mantenha o dispositivo de publicação ativo e conectado.", "Verbinde ein Konto, um Beiträge zu planen. Halte das Gerät zum Veröffentlichen wach und online.", "Connectez un compte pour programmer des publications. Gardez l’appareil de publication éveillé et connecté.", "اربط حسابًا لجدولة المنشورات. أبقِ جهاز النشر نشطًا ومتصلًا بالإنترنت.", "连接账号以设置帖子排期。请保持发布设备唤醒并联网。", "アカウントを連携して投稿を予約できます。公開するデバイスをスリープさせず、オンラインにしておいてください。"],
    AccountSaved => ["Account saved. Connection setup is still required.", "Cuenta guardada. Aún debes configurar la conexión.", "Conta salva. Ainda é necessário configurar a conexão.", "Konto gespeichert. Die Verbindung muss noch eingerichtet werden.", "Compte enregistré. La connexion reste à configurer.", "تم حفظ الحساب. ما زال إعداد الربط مطلوبًا.", "账号已保存。仍需设置连接。", "アカウントを保存しました。連携の設定がまだ必要です。"],
    RemoveChannel => ["Remove account", "Eliminar cuenta", "Remover conta", "Konto entfernen", "Retirer le compte", "إزالة الحساب", "移除账号", "アカウントを削除"],
    ConfirmDelete => ["Delete this post?", "¿Eliminar esta publicación?", "Excluir esta publicação?", "Diesen Beitrag löschen?", "Supprimer cette publication ?", "هل تريد حذف هذا المنشور؟", "删除此帖子？", "この投稿を削除しますか？"],
    Cancel => ["Cancel", "Cancelar", "Cancelar", "Abbrechen", "Annuler", "إلغاء", "取消", "キャンセル"],
    Retry => ["Retry", "Reintentar", "Tentar novamente", "Erneut versuchen", "Réessayer", "إعادة المحاولة", "重试", "再試行"],
    Thread => ["Thread", "Hilo", "Sequência", "Thread", "Fil de publications", "سلسلة منشورات", "串帖", "スレッド"],
    SinglePost => ["Single post", "Publicación única", "Publicação única", "Einzelner Beitrag", "Publication unique", "منشور واحد", "单条帖子", "単独の投稿"],
    Characters => ["Characters", "Caracteres", "Caracteres", "Zeichen", "Caractères", "الأحرف", "字符数", "文字数"],
    NoWorkspace => ["Open a workspace to use Socials.", "Abre un espacio de trabajo para usar Redes sociales.", "Abra um espaço de trabalho para usar Redes sociais.", "Öffne einen Arbeitsbereich, um Social Media zu nutzen.", "Ouvrez un espace de travail pour utiliser Réseaux sociaux.", "افتح مساحة عمل لاستخدام التواصل الاجتماعي.", "打开工作区以使用社交媒体功能。", "ソーシャルを使うにはワークスペースを開いてください。"],
    StorageUnavailable => ["Socials could not open its saved data. Try again.", "Redes sociales no pudo abrir los datos guardados. Inténtalo de nuevo.", "Redes sociais não conseguiu abrir os dados salvos. Tente novamente.", "Social Media konnte die gespeicherten Daten nicht öffnen. Versuche es erneut.", "Réseaux sociaux n’a pas pu ouvrir les données enregistrées. Réessayez.", "تعذر فتح البيانات المحفوظة للتواصل الاجتماعي. أعد المحاولة.", "无法打开社交媒体的已保存数据。请重试。", "ソーシャルの保存データを開けませんでした。再試行してください。"],
    ChangedElsewhere => ["This post changed elsewhere. Reload before saving.", "Esta publicación cambió en otro lugar. Vuelve a cargarla antes de guardar.", "Esta publicação foi alterada em outro lugar. Recarregue antes de salvar.", "Dieser Beitrag wurde andernorts geändert. Lade ihn vor dem Speichern neu.", "Cette publication a été modifiée ailleurs. Rechargez-la avant d’enregistrer.", "تغير هذا المنشور في مكان آخر. أعد تحميله قبل الحفظ.", "此帖子已在别处更改。保存前请重新加载。", "この投稿は別の場所で変更されました。保存する前に再読み込みしてください。"],
    ProviderSetup => ["Provider setup", "Configuración del proveedor", "Configuração do provedor", "Anbieter einrichten", "Configuration du fournisseur", "إعداد المزوّد", "服务商设置", "サービスの設定"],
    ClientId => ["Client ID", "ID de cliente", "ID do cliente", "Client-ID", "ID client", "معرّف العميل", "客户端 ID", "クライアント ID"],
    RedirectUri => ["Redirect URI", "URI de redirección", "URI de redirecionamento", "Weiterleitungs-URI", "URI de redirection", "عنوان إعادة التوجيه", "重定向 URI", "リダイレクト URI"],
    ConnectionPending => ["Connection pending", "Conexión pendiente", "Conexão pendente", "Verbindung ausstehend", "Connexion en attente", "الربط قيد الانتظار", "等待连接", "連携待ち"],
    DeliveryDetails => ["Publication details", "Detalles de publicación", "Detalhes da publicação", "Veröffentlichungsdetails", "Détails de publication", "تفاصيل النشر", "发布详情", "公開の詳細"],
}

/// Look up a Socials string without allocation or a language fallback.
pub fn t(lang: Lang, key: Key) -> &'static str {
    ROWS[key as usize][lang.column()]
}

#[cfg(test)]
mod tests;
