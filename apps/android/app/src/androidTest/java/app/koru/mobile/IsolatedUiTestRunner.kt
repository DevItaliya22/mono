// SPDX-License-Identifier: MPL-2.0

package app.koru.mobile

import android.app.Application
import android.content.Context
import androidx.test.runner.AndroidJUnitRunner

/** UI fixtures must never initialize KoruApplication's Store, vault or account adapter. */
class IsolatedUiTestRunner : AndroidJUnitRunner() {
    override fun newApplication(cl: ClassLoader, className: String, context: Context): Application =
        super.newApplication(cl, Application::class.java.name, context)
}
